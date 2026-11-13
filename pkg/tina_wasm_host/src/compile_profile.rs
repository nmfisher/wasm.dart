/*
The single source of truth for the wasmtime compile profile of the
packaged components (engine + TUI), shared by the runtime host and
the build-time precompiler.

This file is compiled twice: once as a module of the lib crate and
once included verbatim into `build.rs` (via `include!`). It must
therefore stay free of `crate::` paths — `wasmtime` and `std` only —
and of `//!` inner doc comments (illegal mid-file after `include!`).

The profile is a load-bearing invariant of the precompiled artifacts:
the output of `Engine::precompile_component` deserializes ONLY into
an Engine built from an identical profile. Wasmtime embeds its own
serialization version plus a hash of the compiler settings in every
artifact and validates both at load (`serialization::check_compatible`),
so a mismatched artifact is a load error — never a wrong-code load.
Adding a flag here re-keys the artifacts automatically; the runtime
treats a failed deserialize as "compile from source" (see
`runner::prepare`).

`epoch` differs per leg (TUI true, engine false) and is compile
visible, so the two legs produce different artifact keys on purpose.
*/

use wasmtime::Config;

/// The host triple as a wasmtime target string, or `None` off the
/// supported set (then wasmtime's native default applies and artifacts
/// stay build-machine-local).
fn host_triple() -> Option<&'static str> {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "linux") => Some("x86_64-unknown-linux-gnu"),
        ("aarch64", "linux") => Some("aarch64-unknown-linux-gnu"),
        ("x86_64", "macos") => Some("x86_64-apple-darwin"),
        ("aarch64", "macos") => Some("aarch64-apple-darwin"),
        _ => None,
    }
}

/// The component compile profile for one packaged component.
pub fn component_config(epoch: bool) -> Config {
    let mut config = Config::default();
    config.wasm_gc(true);
    config.wasm_function_references(true);
    config.wasm_exceptions(true);
    // The components' exports are async component functions: driving
    // their tasks (print lowers to a wasi:cli/stdout stream write, a
    // timer to a wasi:clocks wait-for subtask) needs the concurrent
    // component API and its async builtins.
    config.wasm_component_model_async(true);
    config.wasm_component_model_more_async_builtins(true);
    if epoch {
        // The TUI leg's parked-guest kill: an expired deadline traps.
        // Compile-visible: epoch checks are emitted into the code.
        config.epoch_interruption(true);
    }
    // Explicit triple, not the default: a configured target disables the
    // inference of the BUILD machine's CPU features, so the serialized
    // artifacts stay loadable on every machine of the same OS+arch
    // instead of only the builder's CPU. Both `build.rs` and the runtime
    // call this same function, so the compile-settings hash agrees
    // whenever the binary is intact; a cross-arch copy still fails
    // `check_compatible` and falls back to compiling from source.
    if let Some(triple) = host_triple() {
        if let Err(error) = config.target(triple) {
            panic!("unsupported wasmtime target triple {triple}: {error}");
        }
    }
    config
}
