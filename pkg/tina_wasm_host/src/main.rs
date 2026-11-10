//! The packaged Tina host: the engine component compiled in
//! (`TINA_WASM_HOST_ENGINE` at build time), booted with zero flags like
//! native `tina` — the current directory as the read-write workspace,
//! the user's `~/.tina/config` as read-only settings (see
//! `docs/capability_merger.md`). The session flags (`--capabilities`,
//! `--preopen`, `--env`, `--plugin-*`, `--no-stdin`) keep working and
//! fill in what is missing. There is no engine selection: engine
//! updates require rebuilding the host.

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
        let flags = tina_wasm_host::runner::parse_flags(env::args().skip(1))?;
        let embedded = include_bytes!(concat!(env!("OUT_DIR"), "/engine.component.wasm"));
        wasmtime::ensure!(
            !embedded.is_empty(),
            "no engine component embedded in this build; rebuild with \
             TINA_WASM_HOST_ENGINE=<component> and cargo build (~2 minutes incremental)"
        );
        tina_wasm_host::runner::run_guest(embedded.to_vec(), flags).await
    })
}
