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

fn main() -> wasmtime::Result<()> {
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
            let embedded = include_bytes!(concat!(env!("OUT_DIR"), "/engine.component.wasm"));
            wasmtime::ensure!(
                !embedded.is_empty(),
                "no engine component embedded in this build; rebuild with \
                 TINA_WASM_HOST_ENGINE=<component> and cargo build (~2 minutes incremental)"
            );
            tina_wasm_host::runner::run_guest(embedded.to_vec(), flags).await
        } else {
            let embedded = include_bytes!(concat!(env!("OUT_DIR"), "/tui.component.wasm"));
            wasmtime::ensure!(
                !embedded.is_empty(),
                "no TUI component embedded in this build; rebuild with \
                 TINA_WASM_HOST_TUI=<component> and cargo build (~2 minutes incremental)"
            );
            tina_wasm_host::tui::run_bytes(embedded).await
        }
    })
}
