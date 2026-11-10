//! The case runner: executes ONE wasm component under the
//! `wasmdart:tests` case protocol (result-collector in, tested-module
//! out) — the harness the repository's Dart-VM-vs-wasm parity suite
//! drives. The packaged `tina-wasm-host` is embedded-only (its engine
//! is compiled in); this binary exists for the case suite, which feeds
//! arbitrary case components as its single positional argument.

use std::{env, fs};

fn main() -> wasmtime::Result<()> {
    // Wasmtime's concurrent (async component model) internals log under the
    // `wasmtime` target; `RUST_LOG=wasmtime=trace` is invaluable when a guest
    // stalls in the event loop.
    let _ = env_logger::try_init();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let all: Vec<String> = env::args().skip(1).collect();
        // The single positional is the case component; everything else is
        // a session flag.
        let positional: Vec<String> = all
            .iter()
            .filter(|a| !(a.starts_with('-') && a.len() > 1))
            .cloned()
            .collect();
        let flag_args: Vec<String> = all
            .into_iter()
            .filter(|a| a.starts_with('-') && a.len() > 1)
            .collect();
        if positional.len() > 1 {
            wasmtime::bail!("only one case component path");
        }
        let Some(path) = positional.first() else {
            wasmtime::bail!(
                "usage: tina-case-runner <component.wasm> [--preopen=guest=host]... \
                 [--env=NAME=VALUE]... [--no-stdin]"
            );
        };
        let flags = tina_wasm_host::runner::parse_flags(flag_args.into_iter())?;
        let bytes = fs::read(path)
            .map_err(|e| wasmtime::Error::msg(format!("Reading {path}: {e}")))?;
        tina_wasm_host::runner::run_guest(bytes, flags).await
    })
}
