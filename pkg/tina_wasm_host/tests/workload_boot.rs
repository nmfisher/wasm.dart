//! The big-workspace boot regression test (macOS arm64 OOM class).
//!
//! Real-world incident: booting from a large workspace died with
//! wasmtime's `GcHeapOutOfMemory: no capacity for allocation of
//! ~50,000,000 bytes` inside `PreloadedAppIo.load` - the boot walk reads
//! EVERY regular file under the workspace `.tina` (depth 4) fully into
//! memory and `String.fromCharCodes` copies it, so one big session file
//! means multi-MB single allocations at BOOT time, before the first
//! turn. Tool-time file reads (`readFileBytes`) are the same allocation
//! class. A fixed boot-sized heap can never cover arbitrary workspaces,
//! so the host pins an initial heap + a reservation-backed ceiling (see
//! `src/compile_profile.rs`); this test keeps that honest.
//!
//! Boots the REAL embedded host binary against a workspace whose `.tina`
//! carries a 64 MB session file and asserts the engine still reaches
//! `ready` with no BOOT-ERROR. Requires the binary in
//! `TINA_DISCOVERY_RUNNER` (same contract as `discovery_boot.rs`);
//! without it the test skips so plain `cargo test` stays green:
//!
//! ```sh
//! TINA_WASM_HOST_ENGINE=$ENGINE CARGO_TARGET_DIR=/tmp/embedded-target \
//!   cargo build -p tina-wasm-host
//! TINA_DISCOVERY_RUNNER=/tmp/embedded-target/debug/tina-wasm-host \
//!   cargo test --offline --test workload_boot
//! ```

use std::io::Write as _;
use std::process::{Command, Stdio};

/// The synthetic big file: JSON-lines-shaped so it looks like a session
/// store; sized past the largest reported real-world failure (50 MB).
const BIG_FILE_BYTES: usize = 64 << 20;

fn runner() -> Option<std::path::PathBuf> {
    std::env::var_os("TINA_DISCOVERY_RUNNER").map(std::path::PathBuf::from)
}

#[test]
fn big_workspace_tina_file_boots_to_ready() {
    let Some(runner) = runner() else {
        eprintln!(
            "skipping: set TINA_DISCOVERY_RUNNER to an embedded tina-wasm-host \
             (TINA_WASM_HOST_ENGINE=<engine.wasm> cargo build -p tina-wasm-host)"
        );
        return;
    };
    assert!(runner.is_file(), "TINA_DISCOVERY_RUNNER is not a file");

    let home = std::env::temp_dir().join(format!("tina-workload-home-{}", std::process::id()));
    let workspace =
        std::env::temp_dir().join(format!("tina-workload-ws-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&workspace);
    std::fs::create_dir_all(home.join(".tina")).expect("home .tina dir");
    std::fs::create_dir_all(workspace.join(".tina")).expect("workspace .tina dir");

    std::fs::write(
        home.join(".tina").join("config"),
        "version = 1\n\n[default]\nmodel = \"anthropic/claude-sonnet-4-6\"\n",
    )
    .expect("config file");
    // The big session file under the WORKSPACE .tina: the boot walk's
    // depth-4 sweep must read it fully without exhausting the GC heap.
    let line = format!("{{\"k\":\"{}\"}}\n", "x".repeat(98));
    let big = workspace.join(".tina").join("big-session.jsonl");
    {
        let mut file = std::fs::File::create(&big).expect("big file create");
        let mut written = 0usize;
        while written < BIG_FILE_BYTES {
            file.write_all(line.as_bytes()).expect("big file write");
            written += line.len();
        }
    }
    assert!(big.metadata().unwrap().len() >= BIG_FILE_BYTES as u64);

    let output = Command::new(&runner)
        .arg("--headless")
        .current_dir(&workspace)
        .env("HOME", &home)
        .stdin(Stdio::null())
        .output()
        .expect("the host binary runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&workspace);

    assert!(
        !stdout.contains("BOOT-ERROR"),
        "boot must not fail on a big workspace file\nstdout tail: {}\nstderr: {}",
        stdout.lines().rev().take(8).collect::<Vec<_>>().join("\n"),
        stderr
    );
    assert!(
        stdout.contains("\"type\":\"ready\""),
        "the session must reach ready with the big .tina file present\n\
         stdout tail: {}\nstderr: {}",
        stdout.lines().rev().take(8).collect::<Vec<_>>().join("\n"),
        stderr
    );
}
