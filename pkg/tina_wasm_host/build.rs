//! Build-time embedding of the Tina engine component (issue #85 packaging).
//!
//! Setting `TINA_WASM_HOST_ENGINE` to a component file at build time embeds
//! those bytes into the binary (`include_bytes!` of a file in `OUT_DIR`), so
//! one executable carries both host and engine. Without the variable the
//! build embeds a placeholder and the binary requires `--engine=<path>` (or
//! a positional path) at startup. Plugin components are never embedded:
//! they are the dynamic inputs.

use std::{env, fs, path::Path};

fn main() {
    println!("cargo:rerun-if-env-changed=TINA_WASM_HOST_ENGINE");
    let out = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join("engine.component.wasm");
    match env::var_os("TINA_WASM_HOST_ENGINE") {
        Some(path) => {
            fs::copy(&path, &out)
                .unwrap_or_else(|error| panic!("copying embedded engine {path:?}: {error}"));
        }
        None => fs::write(&out, b"").expect("writing the empty engine placeholder"),
    }
}
