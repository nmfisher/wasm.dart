//! The `--tui` mode (Option 2 spike, issue #85): run a wasm TUI component
//! against the real terminal. The host owns the tty — raw mode, window size,
//! byte transport and restoration — while the guest draws ANSI bytes and
//! parses stdin bytes. Spike scope: not production, no plugin/engine wiring.
//!
//! Contract: `tina-wasm-host --tui <component.wasm>` puts the terminal in raw
//! mode BEFORE the stdin pump starts, forwards process stdin bytes to the
//! guest and guest stdout bytes to the terminal, answers `tina:term/tty`
//! `get-size` with the live window (so the guest's poll sees resizes), and
//! restores the terminal when `run` returns cleanly, when it traps, and when
//! an epoch deadline kills a parked guest (`TINA_TUI_EPOCH_TICKS`).
use std::{env, io::Read as _, time::Duration};

use wasmtime::{
    Config,
    Engine,
    StoreContextMut,
    WasmBacktrace,
    Result,
    Store,
    component::{Component, Linker, Val},
};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView, p3, cli::AsyncStdinStream};

/// Host state for the wasi host functions: the WASI configuration and the
/// resource table its streams live in. Nothing else — the TUI guest imports
/// no tina:http / tina:process / plugin surfaces.
struct TuiHostState {
    wasi: WasiCtx,
    table: ResourceTable,
}

impl WasiView for TuiHostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

/// The live terminal window size, stdout first, stdin as fallback.
pub fn window_size() -> Result<(u32, u32)> {
    for fd in [1, 0] {
        // SAFETY: winsize is a plain POD struct and the ioctl only fills it.
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        // SAFETY: querying the window size of our own std fds.
        let ok = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } == 0;
        if ok && ws.ws_col > 0 && ws.ws_row > 0 {
            return Ok((ws.ws_col as u32, ws.ws_row as u32));
        }
    }
    Err(wasmtime::Error::msg("no terminal window size (not a tty?)"))
}

/// Raw mode on stdin, restored on [Drop] — including across traps and epoch
/// kills, which surface as `Err` from the guest call but still unwind here.
struct RawMode {
    saved: libc::termios,
    active: bool,
}

impl RawMode {
    fn enable() -> Result<Self> {
        // SAFETY: tcgetattr/tcsetattr on our own stdin fd with a valid termios.
        let saved: libc::termios = unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            wasmtime::ensure!(
                libc::tcgetattr(0, &mut t) == 0,
                "tcgetattr failed: stdin is not a terminal"
            );
            t
        };
        let mut raw = saved;
        // SAFETY: cfmakeraw only adjusts the copy's flags.
        unsafe { libc::cfmakeraw(&mut raw) };
        wasmtime::ensure!(
            // SAFETY: applying raw mode to our own stdin fd.
            unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw) } == 0,
            "tcsetattr (raw mode) failed"
        );
        Ok(Self {
            saved,
            active: true,
        })
    }

    fn restore(&mut self) {
        if self.active {
            // SAFETY: restoring the saved termios on our own stdin fd.
            unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.saved) };
            self.active = false;
        }
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Run the TUI component over the real terminal. Returns once the guest's
/// `run` export completes (or fails); the terminal is always restored first.
/// Spike scope retained through the production rewire: the guest imports
/// only WASI + `tina:term/tty` here; the production TUI guest's tina host
/// imports, discovery preopens and failure channel are the implementation
/// milestone (`docs/wasm_tui.md`).
pub async fn run_bytes(bytes: &[u8]) -> Result<()> {
    let mut config = Config::default();
    config.wasm_gc(true);
    config.wasm_function_references(true);
    config.wasm_exceptions(true);
    config.wasm_component_model_async(true);
    // Timers are wait-for subtasks: without this the clock import is a stub.
    config.wasm_component_model_more_async_builtins(true);
    // Epoch interruption: the ticker below keeps the guest alive, an
    // opt-in deadline (`TINA_TUI_EPOCH_TICKS`) makes a parked guest's next
    // waitable trap — the forced-kill restore case.
    config.epoch_interruption(true);
    let engine = Engine::new(&config)?;
    // With epoch interruption enabled the ticker must run from the very
    // start: instantiation itself is interruptible, so a late ticker traps
    // the guest before the first frame.
    {
        let tick_engine = engine.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(10));
            tick_engine.increment_epoch();
        });
    }

    let component = Component::new(&engine, &bytes)?;

    // Raw mode before the stdin pump starts: the pump must see raw bytes.
    let mut raw = RawMode::enable()?;
    let (cols, rows) = window_size()?;

    let stdout = crate::runner::TrackedStdout::default();
    let mut wasi = WasiCtxBuilder::new();
    wasi.stdout(stdout.clone());
    // The initial size also rides the environment (the tty import stays the
    // live authority — the env snapshot cannot follow a resize).
    wasi.env("TINA_TUI_COLS", &cols.to_string());
    wasi.env("TINA_TUI_ROWS", &rows.to_string());
    // `tokio::io::stdin` blocks the single-threaded runtime; a plain thread
    // pumps process stdin into a channel (the engine mode's pattern). The
    // watchdog keeps a clone of the sender (see below).
    let (tx, rx) = tokio::sync::mpsc::channel::<bytes::Bytes>(16);
    let watchdog_tx = tx.clone();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buffer = [0u8; 8192];
        loop {
            match stdin.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let chunk = bytes::Bytes::copy_from_slice(&buffer[..n]);
                    if tx.blocking_send(chunk).is_err() {
                        break;
                    }
                }
            }
        }
    });
    wasi.stdin(AsyncStdinStream::new(crate::runner::ChannelReader { receiver: rx }));

    let mut store = Store::new(
        &engine,
        TuiHostState {
            wasi: wasi.build(),
            table: ResourceTable::new(),
        },
    );
    // With epoch interruption on, an unset deadline is 0 — every tick traps.
    // Give instantiation and the run a huge budget; the opt-in kill below
    // replaces it right before the guest call. Async stores default the
    // deadline handler to yield-and-extend; make deadlines trap instead.
    store.epoch_deadline_trap();
    store.set_epoch_deadline(10_000_000_000);

    let mut linker = Linker::new(&engine);
    p3::add_to_linker::<TuiHostState>(&mut linker)?;
    // The dart2wasm runtime's backtrace capture (wasm:dart/trace@1.0.0):
    // render the wasm frames on the host's own stack, the same way the
    // engine mode does.
    {
        let mut root = linker.root();
        let mut trace = root.instance("wasm:dart/trace@1.0.0")?;
        trace.func_wrap(
            "capture-utf16",
            |store: StoreContextMut<'_, TuiHostState>, _capacity: (u32,)| {
                let backtrace = WasmBacktrace::force_capture(&store);
                let frames: Vec<String> = backtrace
                    .frames()
                    .iter()
                    .skip(3)
                    .map(|frame| crate::runner::render_frame(frame))
                    .collect();
                let mut text = String::from("Uncaught\n");
                for frame in frames {
                    text.push_str("    at ");
                    text.push_str(&frame);
                    text.push('\n');
                }
                Ok((text,))
            },
        )?;
    }
    {
        let mut root = linker.root();
        let mut tty = root.instance("tina:term/tty@0.1.0")?;
        tty.func_wrap(
            "get-size",
            |_store, _params: ()| -> Result<((u32, u32),)> {
                Ok((window_size()?,))
            },
        )?;
    }

    let instance = linker
        .instantiate_async(&mut store, &component)
        .await
        .map_err(|e| {
            eprintln!("tui host: instantiate failed: {e}");
            e
        })?;
    let Some(run_instance_index) = component.get_export_index(None, "wasi:cli/run@0.3.0") else {
        wasmtime::bail!("TUI component exports no wasi:cli/run");
    };
    let Some(run_idx) = component.get_export_index(Some(&run_instance_index), "run") else {
        wasmtime::bail!("TUI component's wasi:cli/run exports no run");
    };
    let run_fn = instance.get_func(&mut store, run_idx).unwrap();

    // Opt-in forced kill, two mechanisms:
    // - TINA_TUI_EPOCH_TICKS: after this many ticks (10ms each) a guest that
    //   wakes into the deadline traps — the epoch kill of a parked guest.
    // - TINA_TUI_KILL_MS: a host watchdog yanks the guest after this long
    //   no matter what state it is in (also covers a guest task that died
    //   without ever returning its result).
    let kill_ms: Option<u64> = env::var("TINA_TUI_KILL_MS")
        .ok()
        .and_then(|spec| spec.parse().ok());
    if let Some(ms) = kill_ms {
        eprintln!("tui host: watchdog armed for {ms}ms");
    }
    if let Ok(spec) = env::var("TINA_TUI_EPOCH_TICKS") {
        match spec.parse::<u64>() {
            Ok(ticks) if ticks > 0 => {
                eprintln!("tui host: epoch deadline in {ticks} ticks (~{}ms)", ticks * 10);
                store.set_epoch_deadline(ticks);
            }
            _ => {}
        }
    }

    // The watchdog cannot just drop the store: parked concurrent guest tasks
    // make the drop a deadlock. Instead it injects Ctrl-C into the guest's
    // stdin — the host owns the byte stream, so this is a host-initiated
    // quit, and the guest still tears down cleanly.
    if let Some(ms) = kill_ms {
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            eprintln!("tui host: watchdog fired after {ms}ms; injecting Ctrl-C");
            let _ = watchdog_tx
                .send(bytes::Bytes::from_static(b"\x03"))
                .await;
        });
    }

    let body = store.run_concurrent(async |accessor| -> Result<()> {
        let mut results = [Val::Result(Ok(None))];
        run_fn.call_concurrent(accessor, &[], &mut results).await?;
        // Wait out stdout writes the guest left in flight, like the
        // engine mode does between cases.
        while stdout.outstanding() > 0 {
            tokio::task::yield_now().await;
        }
        Ok(())
    });
    let outcome = body.await;

    raw.restore();
    match outcome {
        // The run_concurrent result: the outer Result is the host-side call
        // machinery; the inner one is the guest's `run` result (a trap or
        // epoch kill lands here as Err).
        //
        // host-side call machinery; the inner one is the guest's `run`
        // result (a trap/epoch kill lands here as Err).
        Ok(Ok(())) => {
            eprintln!("tui host: guest run() returned cleanly; terminal restored");
            Ok(())
        }
        Ok(Err(guest)) => {
            eprintln!("tui host: guest run() FAILED ({guest}); terminal restored");
            // Distinct exit code for the demo, only after the terminal is
            // back in cooked mode.
            std::process::exit(3);
        }
        Err(host) => {
            eprintln!("tui host: guest call failed ({host}); terminal restored");
            std::process::exit(3);
        }
    }
}
