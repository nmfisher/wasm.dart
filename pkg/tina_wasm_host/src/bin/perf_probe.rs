//! Performance probe for the plugin boundary (issue #85 measurements).
//!
//! Reports per-call overhead, load (compile+instantiate) time, epoch-kill
//! latency, and unload reclamation for the real plugin fixture. Not a test:
//! run explicitly, e.g.
//!
//!   cargo run --bin perf_probe -- \
//!     testdata/plugin_probe/tina_prompt_plugin.component.wasm \
//!     testdata/plugin_probe/spin.component.wasm [calls] [cycles]
//!
//! The issue sets no numeric budgets: this prints measured numbers, it does
//! not assert thresholds.
use std::{
    time::{Duration, Instant},
};

use tina_wasm_host::plugin_loader::{
    PluginBoundary, PluginCallFrame, PluginLimits, plugin_engine_config,
};

fn rss_bytes() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").expect("statm");
    let pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .expect("resident field")
        .parse()
        .expect("resident pages");
    pages * 4096
}

fn stats(samples: &[Duration]) -> (f64, f64, f64) {
    let mut micros: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1e6).collect();
    micros.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = micros.iter().sum::<f64>() / micros.len() as f64;
    let pct = |q: f64| micros[((micros.len() as f64 * q).round() as usize).min(micros.len() - 1)];
    (mean, pct(0.5), pct(0.95))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: perf_probe <plugin.component.wasm> <spin.component.wasm> [calls=200] [cycles=30]");
        std::process::exit(2);
    }
    let calls: usize = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(200);
    let cycles: usize = args.get(4).and_then(|v| v.parse().ok()).unwrap_or(30);
    let plugin_bytes = std::fs::read(&args[1]).expect("plugin component");
    let spin_bytes = std::fs::read(&args[2]).expect("spin component");

    println!(
        "{{\"event\":\"identities\",\"host\":\"{}\",\"profile\":\"{}\"}}",
        env!("CARGO_PKG_VERSION"),
        if cfg!(debug_assertions) { "debug" } else { "release" },
    );

    // --- compile (Component::new only) ---
    let engine = wasmtime::Engine::new(&plugin_engine_config()).unwrap();
    let started = Instant::now();
    let component = wasmtime::component::Component::new(&engine, &plugin_bytes).unwrap();
    let first_compile = started.elapsed();
    let mut recompiles = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        drop(wasmtime::component::Component::new(&engine, &plugin_bytes).unwrap());
        recompiles.push(started.elapsed());
    }
    drop(component);
    println!(
        "{{\"event\":\"compile\",\"first_ms\":{:.2},\"repeat_mean_ms\":{:.2}}}",
        first_compile.as_secs_f64() * 1e3,
        recompiles.iter().map(|d| d.as_secs_f64() * 1e3).sum::<f64>() / recompiles.len() as f64,
    );

    // --- boundary lifecycle on the real plugin ---
    let broker: std::sync::Arc<dyn tina_wasm_host::plugin_loader::PluginServiceBroker> =
        std::sync::Arc::new(tina_wasm_host::plugin_loader::EchoBroker);
    let mut boundary = PluginBoundary::with_limits(
        engine.clone(),
        broker.clone(),
        PluginLimits::default(),
    )
    .unwrap();
    let policy = tina_wasm_host::plugin_loader::PluginPolicy {
        allow_engine_service_import: true,
        allow_engine_service_call: true,
    };
    let started = Instant::now();
    let handle = boundary.load(&plugin_bytes, policy).unwrap();
    let load_total = started.elapsed();
    let descriptor = boundary.describe(handle).unwrap();
    println!(
        "{{\"event\":\"load\",\"compile_instantiate_ms\":{:.2},\"id\":\"{}\",\"description\":\"{}\"}}",
        load_total.as_secs_f64() * 1e3,
        descriptor.id,
        descriptor.description.replace('"', "'"),
    );

    // --- per-call overhead: run with the granted service round trip ---
    let mut samples = Vec::new();
    for i in 0..calls {
        let frame = PluginCallFrame {
            session_id: "perf-session".into(),
            model: "perf-model".into(),
        };
        let started = Instant::now();
        let result = boundary.run_with_frame(handle, &format!("ping-{i}"), Some(frame));
        let elapsed = started.elapsed();
        match result {
            Ok(Ok(section)) => {
                debug_assert!(!section.is_empty());
            }
            other => panic!("plugin run failed on iteration {i}: {other:?}"),
        }
        samples.push(elapsed);
    }
    let (mean, median, p95) = stats(&samples);
    println!(
        "{{\"event\":\"run_call\",\"calls\":{calls},\"mean_us\":{:.1},\"median_us\":{:.1},\"p95_us\":{:.1}}}",
        mean, median, p95,
    );

    // --- describe per-call overhead ---
    let mut samples = Vec::new();
    for _ in 0..calls {
        let started = Instant::now();
        let descriptor = boundary.describe(handle).unwrap();
        debug_assert!(!descriptor.id.is_empty());
        samples.push(started.elapsed());
    }
    let (mean, median, p95) = stats(&samples);
    println!(
        "{{\"event\":\"describe_call\",\"calls\":{calls},\"mean_us\":{:.1},\"median_us\":{:.1},\"p95_us\":{:.1}}}",
        mean, median, p95,
    );

    // --- unload/reload reclamation: repeated load/run/retire cycles ---
    let before_cycles = rss_bytes();
    let mut load_samples = Vec::new();
    let mut retire_samples = Vec::new();
    let mut cleanup_failures = 0u64;
    let mut handle_previous = handle;
    for _ in 0..cycles {
        let started = Instant::now();
        let next = boundary.load(&plugin_bytes, policy).unwrap();
        load_samples.push(started.elapsed());
        let frame = PluginCallFrame {
            session_id: "perf-session".into(),
            model: "perf-model".into(),
        };
        // The plugin's `run` can legitimately return Err(PluginFailure) as a
        // value; only a trap must abort the probe.
        let outcome = boundary.run_with_frame(next, "warm", Some(frame)).unwrap();
        debug_assert!(outcome.is_ok());
        let started = Instant::now();
        if boundary.retire(handle_previous).is_err() {
            cleanup_failures += 1;
        }
        retire_samples.push(started.elapsed());
        handle_previous = next;
    }
    if boundary.retire(handle_previous).is_err() {
        cleanup_failures += 1;
    }
    let after_cycles = rss_bytes();
    // A final forced GC pass is not available; report the raw resident delta.
    let (load_mean, load_median, _) = stats(&load_samples);
    let (retire_mean, _, _) = stats(&retire_samples);
    println!(
        "{{\"event\":\"unload_cycles\",\"cycles\":{cycles},\"load_mean_ms\":{:.2},\"load_median_ms\":{:.2},\"retire_mean_ms\":{:.2},\"cleanup_failures\":{cleanup_failures},\"rss_before_bytes\":{before_cycles},\"rss_after_bytes\":{after_cycles},\"rss_delta_bytes\":{}}}",
        load_mean / 1e3,
        load_median / 1e3,
        retire_mean / 1e3,
        after_cycles as i64 - before_cycles as i64,
    );

    // --- epoch-kill latency on the spin fixture (short deadline knob) ---
    boundary.set_call_deadline(Duration::from_millis(500));
    let spin = boundary.load(&spin_bytes, policy).unwrap();
    let started = Instant::now();
    let result = boundary.run_with_frame(
        spin,
        "spin",
        Some(PluginCallFrame {
            session_id: "perf-session".into(),
            model: "perf-model".into(),
        }),
    );
    let kill_latency = started.elapsed();
    let killed = result.is_err();
    println!(
        "{{\"event\":\"epoch_kill\",\"killed\":{killed},\"latency_ms\":{:.2}}}",
        kill_latency.as_secs_f64() * 1e3,
    );
    let _ = boundary.retire(spin);

    println!("{{\"event\":\"done\"}}");
}
