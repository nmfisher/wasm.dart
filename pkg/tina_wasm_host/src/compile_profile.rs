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

/// No explicit target triple: native codegen everywhere, deliberately.
///
/// History (f37a247 follow-up): pinning `x86_64-unknown-linux-gnu` made
/// this box's Cranelift ~166x slower at compiling the same engine
/// component (95.5 s cold vs 0.57 s native; the disk-cache warm hit is
/// 575 ms, so the difference is compile time, not the cache), and the
/// macOS arm64 release build that pinned `aarch64-apple-darwin` (baseline
/// aarch64 codegen) WEDGED at boot while the pre-AOT Mac build (native
/// codegen) boots fine. Native is the pre-AOT, validated-on-all-platforms
/// configuration, and deserialization does not care: build.rs and the
/// runtime share this function, so the compile-settings hash agrees, and
/// an artifact built on machine A still cleanly rejects (never
/// wrong-loads) on machine B whose native features differ - the runtime
/// then compiles natively, fast.
fn explicit_triple() -> Option<&'static str> {
    None
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
    // The dart2wasm guest's object heap lives in the wasm-GC heap, which
    // starts at 0 bytes and grows by doubling. Two macOS arm64 incident
    // classes traced here: (1) boot OOM at a 26.7 MB models-cache array,
    // (2) boot OOM at a 50 MB workspace file read (PreloadedAppIo loads
    // every .tina file and String.fromCharCodes copies it - peak is
    // ~3-4x file size) plus a tool-time readFileBytes trap. Both failed
    // with wasmtime's GcHeapOutOfMemory even though the DEFAULT 4 GiB
    // reservation was already in place - i.e. the grow/commit step
    // itself is what fails on macOS arm64; linux grows to 64+ MB
    // workloads without complaint (verified: 64 MB .tina file boots
    // ready). So instead of relying on grow-on-demand:
    // - initial heap 256 MiB: typical workloads (the whole reported
    //   class) allocate inside the pre-existing heap and never hit the
    //   grow path. Accessible-but-untouched pages cost no RAM; RSS
    //   grows with use.
    // - reservation pinned to 4 GiB == wasmtime's hard GC-heap grow cap
    //   (grow_gc_heap saturates at 1<<32): any growth that does happen
    //   is an in-reservation page commit, never a move/remap.
    // All three growth tunables are set because wasmtime switches the
    // others off their memory_* mirrors once any gc_heap_* is explicit.
    // Virtual reservations are lazily committed; only touched pages cost
    // RAM. Note: these tunables are part of the artifact settings hash,
    // and build.rs shares this function, so keys stay consistent.
    config.gc_heap_initial_size(256 << 20);
    config.gc_heap_reservation(4 << 30);
    config.gc_heap_reservation_for_growth(4 << 30);
    config.gc_heap_may_move(true);
    if let Some(triple) = explicit_triple() {
        if let Err(error) = config.target(triple) {
            panic!("unsupported wasmtime target triple {triple}: {error}");
        }
    }
    config
}
