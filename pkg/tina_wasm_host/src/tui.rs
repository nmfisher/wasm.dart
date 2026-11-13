//! The TUI leg of the packaged host (Option 2, issue #85): run the FULL
//! tina TUI component inside the sandbox against the real terminal. The
//! host owns the tty — raw mode enabled before the stdin pump, byte
//! transport, the live `TIOCGWINSZ` size authority (`tina:term/tty`), the
//! crash channel (`tina:term/crash`, spike finding 4), epoch machinery for
//! parked guests (spike findings 1/2), and restoration on every exit
//! path — while the guest draws ANSI bytes, parses stdin bytes and boots
//! the whole session assembly inside the sandbox (discovery preopens,
//! capability policy, plugin runtime: all of `runner::prepare`).
//!
//! Contract: raw mode before the pump; guest stdout bytes to the terminal;
//! `get-size` answered with the live window (the guest polls at 250 ms);
//! terminal restored when `run` returns cleanly, when it traps, when an
//! epoch deadline kills a parked guest, AND when the guest's zone guard
//! reports a crash (`crash.fail` + WASI exit backstop — the host surfaces
//! `guest crashed: <message>` and never parks).
use std::{env, time::Duration};


use wasmtime::{Engine, Result, component::Val};


/// Raw mode on stdin, restored on [Drop] — including across traps and
/// epoch kills, which surface as `Err` from the guest call but still
/// unwind here.
pub(crate) struct RawMode {
    original: libc::termios,
    active: bool,
}

impl RawMode {
    pub(crate) fn enable() -> Result<Self> {
        use std::os::fd::AsRawFd as _;
        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::tcgetattr(std::io::stdin().as_raw_fd(), &mut original) };
        if rc != 0 {
            wasmtime::bail!("tcgetattr failed: stdin is not a terminal");
        }
        let mut raw = original;
        unsafe { libc::cfmakeraw(&mut raw) };
        let rc = unsafe { libc::tcsetattr(std::io::stdin().as_raw_fd(), libc::TCSANOW, &raw) };
        if rc != 0 {
            wasmtime::bail!("tcsetattr (raw mode) failed");
        }
        Ok(Self {
            original,
            active: true,
        })
    }

    fn restore(&mut self) {
        if self.active {
            use std::os::fd::AsRawFd as _;
            unsafe {
                libc::tcsetattr(std::io::stdin().as_raw_fd(), libc::TCSANOW, &self.original);
            }
            self.active = false;
        }
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Run the embedded TUI component over the real terminal with the full
/// runner machinery (discovery, policy, plugins — `runner::prepare`) plus
/// the terminal wrapping. Returns when the guest's `run` export completes
/// or fails; crashes, traps and epoch kills exit non-zero after the
/// terminal is restored.
pub async fn run_bytes(component_bytes: Vec<u8>, flags: crate::runner::GuestFlags) -> Result<()> {
    // Raw mode BEFORE prepare(): the stdin pump starts inside prepare and
    // must see raw bytes.
    let mut raw = RawMode::enable()?;

    let crate::runner::Prepared {
        engine,
        mut store,
        component,
        stdout,
        stdin_tx,
        plugin_runtime,
    } = crate::runner::prepare(&component_bytes, &flags, true, true)?;

    // With epoch interruption on, an unset deadline is 0 — every tick
    // traps. Give instantiation and the run a huge budget; the opt-in kill
    // below replaces it right before the guest call. Async stores default
    // the deadline handler to yield-and-extend; make deadlines trap
    // instead (spike finding 1).
    store.epoch_deadline_trap();
    store.set_epoch_deadline(10_000_000_000);

    // The epoch ticker must run from the very start: instantiation itself
    // is interruptible, so a late ticker traps the guest before the first
    // frame (spike finding 2).
    let tick_engine: Engine = {
        let tick = engine.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(10));
            tick.increment_epoch();
        });
        engine
    };

    let mut linker = crate::runner::build_linker(&tick_engine, &plugin_runtime)?;
    // The tty authority + the crash channel (tina:term): components that
    // do not import tina:term never reach it.
    crate::tina_term::add_to_linker(&mut linker)?;

    let instance = linker
        .instantiate_async(&mut store, &component)
        .await
        .map_err(|e| {
            // A guest exception during instantiation routes through the
            // crash channel before the WASI exit backstop: surface the
            // recorded message with the failure (the parked-run select
            // below is not armed yet).
            if let Some(message) = store.data().crash.take() {
                eprintln!("tui host: guest crashed during instantiation: {message}");
            }
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
    // - TINA_TUI_EPOCH_TICKS: after this many ticks (10ms each) a guest
    //   that wakes into the deadline traps — the epoch kill of a parked
    //   guest.
    // - TINA_TUI_KILL_MS: a host watchdog yanks the guest after this long
    //   no matter what state it is in (covers a wedged guest that ignores
    //   input; the crash channel covers the crash case structurally).
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

    // The watchdog cannot drop the store: parked concurrent guest tasks
    // deadlock the drop. It injects Ctrl-C into the guest's stdin instead
    // — the host owns the byte stream, so this is a host-initiated quit
    // and the guest still tears down cleanly.
    if let Some(ms) = kill_ms {
        if let Some(tx) = stdin_tx.clone() {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                eprintln!("tui host: watchdog fired after {ms}ms; injecting Ctrl-C");
                let _ = tx.send(bytes::Bytes::from_static(b"\x03")).await;
            });
        }
    }

    let crash = store.data().crash.clone();
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

    // The crash channel (spike finding 4): select the parked run against
    // the guest's zone-guard `crash.fail` signal, so a guest Dart
    // exception surfaces as a host error instead of parking forever.
    tokio::select! {
        outcome = body => {
            raw.restore();
            match outcome {
                // The outer Result is the host-side call machinery; the
                // guest's `run` result is unit, so a trap or epoch kill
                // lands as the inner Err.
                Ok(Ok(())) => {
                    eprintln!(
                        "tui host: guest run() returned cleanly; terminal restored"
                    );
                    Ok(())
                }
                Ok(Err(guest)) => {
                    // The guest's zone guard calls `crash.fail` BEFORE the
                    // WASI exit backstop, but the backstop trap can resolve
                    // the body in the same poll cycle and win the select.
                    // The recorded message must still surface (milestone-3
                    // failure-channel leg): check the crash slot before
                    // reporting the bare trap.
                    if let Some(message) = store.data().crash.take() {
                        eprintln!("tui host: guest crashed: {message}; terminal restored");
                    } else {
                        eprintln!("tui host: guest run() FAILED ({guest}); terminal restored");
                    }
                    std::process::exit(3);
                }
                Err(host) => {
                    // Same race as the `Ok(Err)` arm: the zone guard's
                    // crash.fail precedes the exit-backstop trap, but the
                    // trap can win the select. Surface the recorded message
                    // when it is present (milestone-3 failure-channel leg).
                    if let Some(message) = store.data().crash.take() {
                        eprintln!("tui host: guest crashed: {message}; terminal restored");
                    } else {
                        eprintln!("tui host: guest call failed ({host}); terminal restored");
                    }
                    std::process::exit(3);
                }
            }
        }
        message = crash.wait() => {
            raw.restore();
            eprintln!("tui host: guest crashed: {message}; terminal restored");
            std::process::exit(3);
        }
    }
}
