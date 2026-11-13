//! The default-discovery boot regression test (orchestrator bug report:
//! bare discovery failed on macOS where a file preopen and a directory
//! preopen behave differently through the Dart config reader).
//!
//! Boots the REAL embedded host binary - the packaged shape, engine
//! compiled in - against a home-like fixture directory and asserts the
//! engine reaches `ready`. Requires the binary path in
//! `TINA_DISCOVERY_RUNNER` (a tina-wasm-host built with
//! `TINA_WASM_HOST_ENGINE=<engine.wasm>`); without it the test skips so
//! plain `cargo test` stays green:
//!
//! ```sh
//! TINA_WASM_HOST_ENGINE=$ENGINE CARGO_TARGET_DIR=/tmp/embedded-target \
//!   cargo build -p tina-wasm-host
//! TINA_DISCOVERY_RUNNER=/tmp/embedded-target/debug/tina-wasm-host \
//!   cargo test --offline --test discovery_boot
//! ```

use std::process::{Command, Stdio};

fn runner() -> Option<std::path::PathBuf> {
    std::env::var_os("TINA_DISCOVERY_RUNNER").map(std::path::PathBuf::from)
}

fn fixture(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tina-discovery-boot-{}-{}",
        label,
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("fixture dir");
    dir
}

/// Boots the host bare (no flags) with HOME pointed at the fixture and
/// the working directory at a project dir, feeding EOF on stdin. The
/// contract: the process exits 0 and the pipes protocol reports the
/// session `ready`.
fn assert_boots_to_ready(home: &std::path::Path, workspace: &std::path::Path) {
    let output = Command::new(runner().expect("runner path"))
        // The wasm-tui round switched the dispatch: bare invocation boots the
        // TUI leg (default), so the discovery/pipes leg passes --headless
        // explicitly (main.rs mode switch takes the first argument).
        // Superseded assertion updated with note: the discovery behavior is
        // unchanged; only its selector is new.
        .arg("--headless")
        .current_dir(workspace)
        .env("HOME", home)
        .stdin(Stdio::null())
        .output()
        .expect("the host binary runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "bare discovery must boot: exit {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        stdout,
        stderr
    );
    assert!(
        stdout.contains("\"type\":\"ready\""),
        "the session must reach ready\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );
}

#[test]
fn bare_discovery_boots_to_ready_against_a_home_fixture() {
    let Some(runner) = runner() else {
        eprintln!(
            "skipping: set TINA_DISCOVERY_RUNNER to an embedded tina-wasm-host \
             (TINA_WASM_HOST_ENGINE=<engine.wasm> cargo build -p tina-wasm-host)"
        );
        return;
    };
    assert!(runner.is_file(), "TINA_DISCOVERY_RUNNER is not a file");

    // The home-like fixture: config file inside ~/.tina, project dir as
    // the workspace. A symlinked config exercises the dotfile-manager
    // layout where the mount must stay ~/.tina (whose `config` entry
    // resolves through the link).
    let home = fixture("home");
    std::fs::create_dir_all(home.join(".tina")).expect("config dir");
    std::fs::write(
        home.join(".tina").join("config"),
        "version = 1\n\n[default]\nmodel = \"anthropic/claude-sonnet-4-6\"\n",
    )
    .expect("config file");
    let workspace = fixture("workspace");

    assert_boots_to_ready(&home, &workspace);

    #[cfg(unix)]
    {
        let elsewhere = fixture("elsewhere");
        std::fs::write(
            elsewhere.join("tina.conf"),
            "version = 1\n\n[default]\nmodel = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .expect("target config");
        std::fs::remove_file(home.join(".tina").join("config")).expect("drop the real config");
        std::os::unix::fs::symlink(
            elsewhere.join("tina.conf"),
            home.join(".tina").join("config"),
        )
        .expect("symlink the config");
        assert_boots_to_ready(&home, &workspace);
        std::fs::remove_dir_all(&elsewhere).ok();
    }

    std::fs::remove_dir_all(&home).ok();
    std::fs::remove_dir_all(&workspace).ok();
}

#[test]
fn bare_discovery_without_a_config_fails_with_the_friendly_message() {
    let Some(runner) = runner() else {
        eprintln!(
            "skipping: set TINA_DISCOVERY_RUNNER to an embedded tina-wasm-host \
             (TINA_WASM_HOST_ENGINE=<engine.wasm> cargo build -p tina-wasm-host)"
        );
        return;
    };

    // An empty home: no ~/.tina at all.
    let home = fixture("empty-home");
    let workspace = fixture("workspace2");
    let output = Command::new(runner)
        // --headless: see the note on the first invocation in this file.
        .arg("--headless")
        .current_dir(&workspace)
        .env("HOME", &home)
        .stdin(Stdio::null())
        .output()
        .expect("the host binary runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "no config must fail, not boot\nstdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let expected = format!(
        "no tina config found at {}",
        home.join(".tina").join("config").display()
    );
    assert!(
        stderr.contains(&expected),
        "the friendly message names the config path\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("run tina once to create one")
            && stderr.contains("--preopen=config=<dir>"),
        "the friendly message carries the guidance\nstderr: {stderr}"
    );

    std::fs::remove_dir_all(&home).ok();
    std::fs::remove_dir_all(&workspace).ok();
}

/// A config with multiple custom providers (each carrying its own
/// base_url) must boot: the snapshot carries the config bytes verbatim,
/// so every custom provider table survives assembly.
#[test]
fn multiple_custom_providers_in_the_config_boot_to_ready() {
    let Some(runner) = runner() else {
        eprintln!("skipping: TINA_DISCOVERY_RUNNER is not set");
        return;
    };
    let home = fixture("custom-providers");
    std::fs::create_dir_all(home.join(".tina")).expect("config dir");
    std::fs::write(
        home.join(".tina").join("config"),
        r#"version = 1

[default]
model = "custom-a/first-model"

[providers.custom-a]
base_url = "https://custom-a.example/api"
api_key = "placeholder-a"
models = ["first-model|First Model"]

[providers.custom-b]
base_url = "https://custom-b.example/api"
api_key = "placeholder-b"
models = ["second-model|Second Model", "third-model"]
"#,
    )
    .expect("config file");
    let workspace = fixture("workspace3");

    assert_boots_to_ready(&home, &workspace);
    std::fs::remove_dir_all(&home).ok();
    std::fs::remove_dir_all(&workspace).ok();
}

/// The orchestrator's repro shape: the config references a provider that
/// is NOT builtin and carries NO base_url - its endpoint comes from the
/// models.dev descriptor catalog. The snapshot must carry the cache (and
/// point the guest's HOME at the config mount) or the engine refuses the
/// provider at boot ("base_url is required for a custom provider").
#[test]
fn a_catalog_backed_provider_without_base_url_boots_to_ready() {
    let Some(runner) = runner() else {
        eprintln!("skipping: TINA_DISCOVERY_RUNNER is not set");
        return;
    };
    let home = fixture("catalog-provider");
    std::fs::create_dir_all(home.join(".tina").join("cache")).expect("cache dir");
    std::fs::write(
        home.join(".tina").join("config"),
        r#"version = 1

[default]
model = "zai-coding-plan/glm-4.6"
"#,
    )
    .expect("config file");
    // Public models.dev-style cache row (never credentials).
    std::fs::write(
        home
            .join(".tina")
            .join("cache")
            .join("models.dev.providers.json"),
        r#"{
  "zai-coding-plan": {
    "npm": "@ai-sdk/openai-compatible",
    "api": "https://api.z.ai/api/coding-plan",
    "name": "Z.ai Coding Plan",
    "env": ["ZAI_CODING_PLAN_API_KEY"],
    "models": {
      "glm-4.6": {"name": "GLM-4.6"}
    }
  }
}"#,
    )
    .expect("cache file");
    let workspace = fixture("workspace4");

    assert_boots_to_ready(&home, &workspace);
    std::fs::remove_dir_all(&home).ok();
    std::fs::remove_dir_all(&workspace).ok();
}
