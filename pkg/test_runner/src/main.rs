use std::{env, fs};

use serde::Serialize;
use wasmtime::{
    Config, Result, Store, StoreContextMut,
    component::{Component, Linker, Val},
    error::Context,
    WasmBacktrace,
};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView, p3};

fn main() -> Result<()> {
    // Wasmtime's concurrent (async component model) internals log under the
    // `wasmtime` target; `RUST_LOG=wasmtime=trace` is invaluable when a guest
    // stalls in the event loop.
    let _ = env_logger::try_init();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async_main())
}

async fn async_main() -> Result<()> {
    let Some(file) = env::args().nth(1) else {
        wasmtime::bail!("Usage: cargo run -- file.wasm")
    };

    let mut config = Config::default();
    config.wasm_gc(true);
    config.wasm_function_references(true);
    config.wasm_exceptions(true);
    // The test export is an async component function: driving its task
    // (print lowers to a wasi:cli/stdout stream write) needs the concurrent
    // component API.
    config.wasm_component_model_async(true);

    let engine = wasmtime::Engine::new(&config)?;
    let mut store = Store::new(
        &engine,
        HostState {
            wasi: WasiCtxBuilder::new().inherit_stdout().build(),
            table: ResourceTable::new(),
        },
    );

    let bytes = fs::read(&file).with_context(|| format!("Reading {file}"))?;
    let component = Component::new(&engine, bytes)?;
    let Some(run_instance_index) = component.get_export_index(None, "wasmdart:tests/tested-module")
    else {
        wasmtime::bail!("Missing tests export");
    };
    let Some(count_test_idx) = component.get_export_index(Some(&run_instance_index), "count-tests")
    else {
        wasmtime::bail!("Missing count-tests");
    };
    let Some(invoke_test_idx) =
        component.get_export_index(Some(&run_instance_index), "invoke-test")
    else {
        wasmtime::bail!("Missing invoke-test");
    };

    let mut linker = Linker::new(&engine);
    {
        let mut root = linker.root();
        let mut collector = root.instance("wasmdart:tests/result-collector")?;
        collector.func_wrap("record-string", |_store, params: (String,)| {
            post_event(&TestEvent::RecordedString { value: params.0 });
            Ok(())
        })?;
        collector.func_wrap("record-double", |_store, params: (f64,)| {
            post_event(&TestEvent::RecordedDouble { value: params.0 });
            Ok(())
        })?;
        collector.func_wrap("record-int", |_store, params: (i64,)| {
            post_event(&TestEvent::RecordedInt { value: params.0 });
            Ok(())
        })?;
        collector.func_wrap("record-bool", |_store, params: (bool,)| {
            post_event(&TestEvent::RecordedBool { value: params.0 });
            Ok(())
        })?;
    }
    // The `dart:trace` dependency: render the wasm frames on the host's own
    // stack into the string the capture import returns. `func_wrap`'s
    // `StoreContextMut` is exactly the "store is on the call stack" state
    // `WasmBacktrace::force_capture` reads frames from, and wasmtime lowers
    // the returned `String` into guest linear memory itself using the
    // canonical memory + realloc options the compiler registered - the host
    // never touches guest memory directly.
    {
        let mut root = linker.root();
        let mut trace = root.instance("wasm:dart/trace@1.0.0")?;
        trace.func_wrap("capture-utf16", |store: StoreContextMut<'_, HostState>, _capacity: (u32,)| {
            let backtrace = WasmBacktrace::force_capture(&store);
            // The top frames are the capture machinery itself (`capture-utf16`
            // lowering, `tryCaptureStackTrace`, `stackTraceGetCurrent`); the
            // frames the trace is about sit below them, so drop the top three
            // like the Dart VM hides its own capture frame.
            let frames: Vec<String> = backtrace
                .frames()
                .iter()
                .skip(3)
                .map(render_frame)
                .collect();
            let mut text = String::from("Uncaught\n");
            for frame in frames {
                text.push_str("    at ");
                text.push_str(&frame);
                text.push('\n');
            }
            Ok((text,))
        })?;
    }
    // The `dart:timeline` dependency: every guest timeline event arrives as
    // one canonical call, and the host writes it as one NDJSON line onto
    // stderr - the only stream that never mixes with the stdout events the
    // run output is read from. The guest allocated the two strings in its
    // own linear memory and frees them after the call returns, so
    // `func_wrap`'s automatic `String` lowering copies them out before the
    // host returns.
    {
        let mut root = linker.root();
        let mut timeline = root.instance("wasm:dart/timeline@1.0.0")?;
        timeline.func_wrap(
            "record-task-event",
            |_store,
             params: (
                u8,
                i32,
                i32,
                String,
                String,
            )| {
                let (event_type, task_id, flow_id, name, arguments_as_json) =
                    params;
                let line = serde_json::json!({
                    "opt": "timeline",
                    "ty": "timelineEvent",
                    "type": event_type,
                    "task": task_id,
                    "flow": flow_id,
                    "name": name,
                    "args": arguments_as_json,
                });
                eprintln!("{line}");
                Ok((true,))
            },
        )?;
    }
    // The compiled components import `wasi:cli/stdout` (print) and
    // `wasi:random/insecure` (Random); without these the instantiation
    // fails with a missing import. `inherit_stdout` routes guest prints to
    // the host's stdout.
    p3::add_to_linker::<HostState>(&mut linker)?;

    let instance = linker.instantiate_async(&mut store, &component).await?;
    let count = instance.get_func(&mut store, count_test_idx).unwrap();
    let invoke = instance.get_func(&mut store, invoke_test_idx).unwrap();

    let num_tests = {
        let mut results = [Val::Result(Ok(None))];
        count.call_async(&mut store, &[], &mut results).await?;
        let Val::U32(count) = results[0] else {
            panic!();
        };
        count
    };

    for i in 0..num_tests {
        post_event(&TestEvent::TestStart { test: i });
        invoke
            .call_async(&mut store, &[Val::U32(i)], &mut [])
            .await
            .unwrap();
        post_event(&TestEvent::TestEnd { test: i });
    }

    Ok(())
}

/// The state the WASI host functions use: the WASI configuration (stdout)
/// and the resource table its streams live in.
struct HostState {
    wasi: WasiCtx,
    table: ResourceTable,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

fn post_event(event: &TestEvent) {
    let formatted = serde_json::to_string(event).unwrap();
    println!("{formatted}")
}

/// Renders one wasm frame the way Node does (`M.<name>`), so a trace
/// captured by this host has the same shape as one captured in the browser
/// or by the case runner's JavaScript host. `func_name` comes from the
/// module's name section, which dart2wasm emits for every function at `-O0`
/// (this suite's optimization level), so a frame is never just an index.
fn render_frame(frame: &wasmtime::FrameInfo) -> String {
    let name = frame.func_name().unwrap_or("<unnamed>");
    format!("M.{}", name)
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum TestEvent {
    #[serde(rename = "start")]
    TestStart { test: u32 },
    #[serde(rename = "end")]
    TestEnd { test: u32 },
    #[serde(rename = "string")]
    RecordedString { value: String },
    #[serde(rename = "double")]
    RecordedDouble { value: f64 },
    #[serde(rename = "int")]
    RecordedInt { value: i64 },
    #[serde(rename = "bool")]
    RecordedBool { value: bool },
}
