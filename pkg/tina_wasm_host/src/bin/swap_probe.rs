//! Engine-level live-swap probe for the compile-on-load dev loop (issue #85).
//!
//! Exercises `TinaPluginRuntime::watching_artifact` end to end, the same
//! runtime object the Dart engine side drives, without restarting or
//! rebuilding the boundary engine:
//!
//!   attach   - first hook attach compiles and loads the artifact
//!   swap     - artifact file replaced by a rebuild (spin fixture); the next
//!              attach picks up the new bytes while the predecessor instance
//!              keeps its Store and state
//!   swap back- artifact restored; a fresh attach serves the original
//!              behavior again
//!   mid-call - artifact replaced while a guest call is in flight; open is
//!              refused Busy without blocking, close is immediate, the
//!              in-flight result is discarded, and the following attach sees
//!              the swapped bytes
//!
//! Not a test: behavior is asserted, latencies are only recorded. Run with
//!
//!   cargo run --bin swap_probe -- \
//!     testdata/plugin_probe/tina_prompt_plugin.component.wasm \
//!     testdata/plugin_probe/spin.component.wasm [deadline_ms=200]
//!
//! Deadlines are shortened from the production default so the epoch kill is
//! quick; no numeric budgets are asserted (issue #85 sets none).
use std::{
    time::{Duration, Instant},
};

use tina_wasm_host::plugin_loader::{PluginCallFrame, PluginLimits, verify_embedder_abi};
use tina_wasm_host::tina_plugin::{TinaPluginError, TinaPluginRuntime, plugin_engine};

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn frame(session_id: &str, model: &str) -> PluginCallFrame {
    PluginCallFrame {
        session_id: session_id.into(),
        model: model.into(),
    }
}

fn ms(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1e3
}

fn expect_ok(result: std::result::Result<String, TinaPluginError>, what: &str) -> String {
    result.unwrap_or_else(|error| panic!("{what} failed: {error:?}"))
}

fn expect_err(result: std::result::Result<String, TinaPluginError>, what: &str) -> TinaPluginError {
    match result {
        Ok(section) => panic!("{what} unexpectedly succeeded: {section:.60}"),
        Err(error) => error,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: swap_probe <plugin.component.wasm> <spin.component.wasm> [deadline_ms=200]");
        std::process::exit(2);
    }
    let deadline_ms: u64 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(200);
    let v1_bytes = std::fs::read(&args[1]).expect("stateful plugin component");
    let v2_bytes = std::fs::read(&args[2]).expect("spin component");
    verify_embedder_abi(&v1_bytes).expect("v1 embedder ABI preflight");
    verify_embedder_abi(&v2_bytes).expect("v2 embedder ABI preflight");

    let rss_bytes = || -> u64 {
        std::fs::read_to_string("/proc/self/statm")
            .ok()
            .and_then(|statm| statm.split_whitespace().nth(1)?.parse().ok())
            .unwrap_or(0)
    };
    let rss_start = rss_bytes();

    println!(
        "{{\"event\":\"identities\",\"host\":\"{}\",\"profile\":\"{}\",\"deadline_ms\":{deadline_ms},\"v1_len\":{},\"v1_fnv64\":\"{:016x}\",\"v2_len\":{},\"v2_fnv64\":\"{:016x}\"}}",
        env!("CARGO_PKG_VERSION"),
        if cfg!(debug_assertions) { "debug" } else { "release" },
        v1_bytes.len(),
        fnv1a64(&v1_bytes),
        v2_bytes.len(),
        fnv1a64(&v2_bytes),
    );

    let dir = std::env::temp_dir().join(format!("issue85-swap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let artifact = dir.join("plugin.component.wasm");
    let limits = || PluginLimits {
        call_deadline: Duration::from_millis(deadline_ms),
        epoch_tick: Duration::from_millis(5),
        ..PluginLimits::default()
    };
    std::fs::write(&artifact, &v1_bytes).expect("initial artifact write");
    let runtime =
        TinaPluginRuntime::watching_artifact(plugin_engine().unwrap(), artifact.clone(), true, limits())
            .expect("watching runtime");

    // --- attach: first hook attach compiles and loads the artifact ---
    let started = Instant::now();
    let lease_v1 = runtime.open().expect("first attach");
    let attach_ms = started.elapsed();
    let started = Instant::now();
    let section = expect_ok(
        runtime.on_prompt(&lease_v1, frame("swap-session", "model-a")),
        "v1 on_prompt",
    );
    let first_call_ms = started.elapsed();
    assert!(section.contains("swap-session") && section.contains("model-a"));
    assert!(section.contains("run=1"), "stateful counter expected: {section:.80}");
    println!(
        "{{\"event\":\"attach\",\"lease\":\"{lease_v1}\",\"open_ms\":{:.2},\"first_call_ms\":{:.2}}}",
        ms(attach_ms),
        ms(first_call_ms),
    );

    // --- swap: artifact replaced by a "rebuild"; next attach picks it up ---
    let started = Instant::now();
    std::fs::write(&artifact, &v2_bytes).expect("swap write");
    let swap_write_ms = started.elapsed();
    let started = Instant::now();
    let lease_v2 = runtime.open().expect("attach after swap");
    let reload_ms = started.elapsed();
    assert_ne!(lease_v1, lease_v2, "each attach gets a fresh lease");
    let started = Instant::now();
    let outcome = expect_err(
        runtime.on_prompt(&lease_v2, frame("swap-session", "model-a")),
        "swapped (spin) on_prompt",
    );
    let kill_ms = started.elapsed();
    assert_eq!(
        outcome,
        TinaPluginError::Failed,
        "the swapped artifact must be live: spin call must hit the epoch deadline"
    );
    // The predecessor instance keeps its original component, Store and state.
    let section = expect_ok(
        runtime.on_prompt(&lease_v1, frame("swap-session", "model-a")),
        "predecessor on_prompt after swap",
    );
    assert!(section.contains("run=2"), "predecessor state survived: {section:.80}");
    println!(
        "{{\"event\":\"swap_hot\",\"write_ms\":{:.2},\"reload_ms\":{:.2},\"kill_ms\":{:.2},\"outcome\":\"epoch_deadline\",\"predecessor\":\"run=2\"}}",
        ms(swap_write_ms),
        ms(reload_ms),
        ms(kill_ms),
    );

    // --- swap back: restored artifact; fresh attach serves v1 again ---
    std::fs::write(&artifact, &v1_bytes).expect("swap-back write");
    let started = Instant::now();
    let lease_v3 = runtime.open().expect("attach after swap back");
    let reload_back_ms = started.elapsed();
    let section = expect_ok(
        runtime.on_prompt(&lease_v3, frame("swap-session", "model-a")),
        "restored v1 on_prompt",
    );
    assert!(section.contains("run=1"), "fresh Store after reload: {section:.80}");
    // The epoch-killed lease was already revoked by the kill; a second close
    // must be refused rather than retiring twice.
    assert_eq!(runtime.close(&lease_v2), Err(TinaPluginError::Denied));
    let close_v1 = runtime.close(&lease_v1);
    let close_v3 = runtime.close(&lease_v3);
    let mut retire_failures = usize::from(close_v1.is_err()) + usize::from(close_v3.is_err());
    println!(
        "{{\"event\":\"swap_back\",\"reload_ms\":{:.2},\"behavior\":\"v1 run=1\",\"close_v1_ok\":{},\"close_v3_ok\":{},\"killed_lease_close\":\"denied\"}}",
        ms(reload_back_ms),
        close_v1.is_ok(),
        close_v3.is_ok(),
    );

    // --- mid-call swap: replace the artifact while a guest call runs ---
    let mid_limits = PluginLimits {
        call_deadline: Duration::from_millis(deadline_ms * 3 / 2),
        epoch_tick: Duration::from_millis(5),
        ..PluginLimits::default()
    };
    std::fs::write(&artifact, &v2_bytes).expect("mid-call swap write");
    let mid_runtime = TinaPluginRuntime::watching_artifact(
        plugin_engine().unwrap(),
        artifact.clone(),
        true,
        mid_limits,
    )
    .expect("mid-call watching runtime");
    let lease_spin = mid_runtime.open().expect("spin attach");
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            let started = Instant::now();
            let result = mid_runtime.on_prompt(&lease_spin, frame("mid-call", "model-a"));
            (started.elapsed(), result)
        });
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(&artifact, &v1_bytes).expect("swap during call");
        // open must refuse Busy without waiting on plugin code.
        let started = Instant::now();
        let open_result = mid_runtime.open();
        let busy_ms = started.elapsed();
        assert!(matches!(open_result, Err(TinaPluginError::Busy)));
        // close must never wait on the guest either; retirement is deferred.
        let started = Instant::now();
        let close_result = mid_runtime.close(&lease_spin);
        let close_ms = started.elapsed();
        assert_eq!(close_result, Ok(()), "close during an active call must succeed");
        let (call_elapsed, call_result) = worker.join().expect("worker");
        assert!(
            matches!(call_result, Err(TinaPluginError::Failed)),
            "an in-flight call whose lease was closed must not surface a result"
        );
        // The boundary is idle again and the swapped bytes are next.
        let started = Instant::now();
        let lease_after = mid_runtime.open().expect("attach after mid-call swap");
        let post_reload_ms = started.elapsed();
        let section = expect_ok(
            mid_runtime.on_prompt(&lease_after, frame("post-swap", "model-a")),
            "post-swap v1 on_prompt",
        );
        assert!(section.contains("run=1"), "{section:.80}");
        if mid_runtime.close(&lease_after).is_err() {
            retire_failures += 1;
        }
        println!(
            "{{\"event\":\"mid_call_swap\",\"call_elapsed_ms\":{:.2},\"call_discarded\":true,\"busy_ms\":{:.2},\"close_ms\":{:.2},\"post_reload_ms\":{:.2},\"post_behavior\":\"v1 run=1\"}}",
            ms(call_elapsed),
            ms(busy_ms),
            ms(close_ms),
            ms(post_reload_ms),
        );
    });

    let rss_end = rss_bytes();
    println!(
        "{{\"event\":\"summary\",\"retire_failures\":{retire_failures},\"rss_start_bytes\":{rss_start},\"rss_end_bytes\":{rss_end},\"rss_delta_bytes\":{}}}",
        rss_end as i64 - rss_start as i64,
    );
    let _ = std::fs::remove_file(&artifact);
    let _ = std::fs::remove_dir(&dir);
    println!("{{\"event\":\"done\"}}");
}
