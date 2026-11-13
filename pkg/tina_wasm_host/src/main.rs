//! The packaged Tina host, Option 2 shape (design-stage merge resolution;
//! the full dispatch is implemented on this branch per
//! `docs/wasm_tui.md` / tina `docs/option2-wasm-tui.md`):
//!
//! - DEFAULT: the full TUI component (embedded with `TINA_WASM_HOST_TUI`
//!   the same way the engine is) runs inside the sandbox against the real
//!   terminal — the host owns the tty.
//! - `--headless` (alias `--pipes`): the pipes engine component for agents
//!   and scripts, embedded with `TINA_WASM_HOST_ENGINE`, booted with zero
//!   flags like native `tina` — the current directory as the read-write
//!   workspace, the user's `~/.tina/config` as read-only settings (see
//!   `docs/capability_merger.md`).
//!
//! The session flags (`--capabilities`, `--preopen`, `--env`, `--plugin-*`,
//! `--no-stdin`) keep working on the headless leg and fill in what is
//! missing. There is no engine or TUI selection by path: component updates
//! require rebuilding the host.

use std::env;

// The embedded components and their precompiled twins. Module-level
// statics on purpose: `include_bytes!` inside a function body creates an
// anonymous static that each codegen unit references separately, and the
// linker keeps every copy - the macOS release build measured 223.6 MB
// (every artifact embedded 3x) until these were hoisted here. Named
// statics are singletons: one copy each (~61 MB total in release).
static ENGINE_COMPONENT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/engine.component.wasm"));
static ENGINE_PRECOMPILED: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/engine.component.cwasm"));
static TUI_COMPONENT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tui.component.wasm"));
static TUI_PRECOMPILED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tui.component.cwasm"));

fn main() -> wasmtime::Result<()> {
    // Anchors the boot-phase trace (TINA_WASM_BOOT_TRACE=1; see
    // `runner::boot_mark`) - the first mark IS t0 for every later one.
    tina_wasm_host::runner::boot_mark("main:start");
    // Wasmtime's concurrent (async component model) internals log under the
    // `wasmtime` target; `RUST_LOG=wasmtime=trace` is invaluable when a guest
    // stalls in the event loop.
    let _ = env_logger::try_init();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        // Mode switch: the first `--headless`/`--pipes` selects the pipes
        // engine; everything else (including `--tui`, the spike's explicit
        // form) keeps the default TUI surface. Remaining arguments stay for
        // the mode's flag parser.
        let mut args = env::args().skip(1).peekable();
        let headless = match args.peek().map(String::as_str) {
            Some("--headless") | Some("--pipes") => {
                args.next();
                true
            }
            Some("--tui") => {
                args.next();
                false
            }
            _ => false,
        };
        if headless {
            let flags = tina_wasm_host::runner::parse_flags(args)?;
            tina_wasm_host::runner::boot_mark("main:flags-parsed");
            let embedded = ENGINE_COMPONENT;
            wasmtime::ensure!(
                !embedded.is_empty(),
                "no engine component embedded in this build; rebuild with \
                 TINA_WASM_HOST_ENGINE=<component> and cargo build (~2 minutes incremental)"
            );
            // The build-time precompiled twin of the component (see
            // build.rs): deserialize instead of JIT-compiling at boot.
            // TINA_WASM_AOT=0 forces the compile path (diagnosis hatch:
            // A/B the deserialize path without rebuilding).
            tina_wasm_host::runner::run_guest(
                embedded.to_vec(),
                tina_wasm_host::runner::aot_bytes(ENGINE_PRECOMPILED),
                flags,
            )
            .await
        } else {
            let flags = tina_wasm_host::runner::parse_flags(args)?;
            let embedded = TUI_COMPONENT;
            wasmtime::ensure!(
                !embedded.is_empty(),
                "no TUI component embedded in this build; rebuild with \
                 TINA_WASM_HOST_TUI=<component> and cargo build (~2 minutes incremental)"
            );
            tina_wasm_host::tui::run_bytes(
                embedded.to_vec(),
                tina_wasm_host::runner::aot_bytes(TUI_PRECOMPILED),
                flags,
            )
            .await
        }
    })
}
