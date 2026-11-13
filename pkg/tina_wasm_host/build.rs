//! Build-time embedding of the Tina components (issue #85 packaging),
//! plus their precompiled twins.
//!
//! Setting `TINA_WASM_HOST_ENGINE` / `TINA_WASM_HOST_TUI` to a component
//! file at build time embeds those bytes into the binary
//! (`include_bytes!` of a file in `OUT_DIR`), so one executable carries
//! host and component together. Without the variable the build embeds a
//! placeholder. Plugin components are never embedded: they are the
//! dynamic inputs.
//!
//! Since the wasm startup work, each embedded component ALSO gets a
//! precompiled artifact (`*.cwasm`): build.rs compiles the component
//! with the exact profile the runtime uses (`src/compile_profile.rs`,
//! included below) and serializes Cranelift's output. At boot the host
//! deserializes (an mmap + relocation) instead of JIT-compiling —
//! the difference between milliseconds and minutes for the 4.8 MB TUI.
//! The artifact is keyed by wasmtime itself: the serialized bytes carry
//! wasmtime's compilation version and a hash of the compile settings,
//! and a load into a differently-configured Engine is rejected (the
//! runtime then compiles from the embedded component as a fallback).

use std::{env, fs, path::Path};

// The compile profile shared with the runtime (see the module doc for
// why this must stay `crate::`-free).
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/compile_profile.rs"
));

fn main() {
    println!("cargo:rerun-if-env-changed=TINA_WASM_HOST_ENGINE");
    let out = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join("engine.component.wasm");
    match env::var_os("TINA_WASM_HOST_ENGINE") {
        Some(path) => {
            let path = Path::new(&path);
            // The env value alone does not trigger a rebuild when it is
            // unchanged: watch the FILE, so a rebuilt engine component is
            // re-embedded instead of silently shipping stale bytes.
            println!("cargo:rerun-if-changed={}", path.display());
            fs::copy(path, &out)
                .unwrap_or_else(|error| panic!("copying embedded engine {path:?}: {error}"));
        }
        None => fs::write(&out, b"").expect("writing the empty engine placeholder"),
    }
    // `epoch` mirrors the leg the component serves: the TUI runner arms
    // epoch deadlines, the engine (pipes) runner does not.
    precompile(&out, "engine.component.cwasm", false);

    // The Option 2 TUI component embeds the same way (docs/wasm_tui.md).
    println!("cargo:rerun-if-env-changed=TINA_WASM_HOST_TUI");
    let tui_out = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join("tui.component.wasm");
    match env::var_os("TINA_WASM_HOST_TUI") {
        Some(path) => {
            let path = Path::new(&path);
            println!("cargo:rerun-if-changed={}", path.display());
            fs::copy(path, &tui_out)
                .unwrap_or_else(|error| panic!("copying embedded TUI component {path:?}: {error}"));
        }
        None => fs::write(&tui_out, b"").expect("writing the empty TUI placeholder"),
    }
    precompile(&tui_out, "tui.component.cwasm", true);
}

/// Serialize the precompiled twin of an embedded component next to it.
/// A placeholder component (or a precompile failure) writes an empty
/// artifact: the runtime sees empty, skips deserialize, and compiles
/// from the embedded component — precompilation is an optimization with
/// a always-safe fallback, never a build blocker.
fn precompile(component_out: &Path, artifact_name: &str, epoch: bool) {
    let artifact_out = component_out.with_file_name(artifact_name);
    let bytes = match fs::read(component_out) {
        Ok(bytes) if !bytes.is_empty() => bytes,
        _ => {
            let _ = fs::write(&artifact_out, b"");
            return;
        }
    };
    let engine = wasmtime::Engine::new(&component_config(epoch))
        .unwrap_or_else(|error| panic!("building the precompile engine: {error}"));
    match engine.precompile_component(&bytes) {
        Ok(artifact) => {
            fs::write(&artifact_out, artifact).unwrap_or_else(|error| {
                panic!("writing the precompiled artifact {artifact_out:?}: {error}")
            });
        }
        Err(error) => {
            eprintln!(
                "cargo:warning=precompiling {} failed ({error}); the host \
                 will JIT-compile it at first boot",
                component_out.display()
            );
            let _ = fs::write(&artifact_out, b"");
        }
    }
}
