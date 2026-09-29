use std::{env, fs};

use serde::Serialize;
use wasmtime::{
    Config, Result, Store,
    component::{Component, Linker, Val},
    error::Context,
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
