use std::{
    env,
    fs,
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    // `AsyncWrite`'s methods take `Pin<&mut Self>`.
    pin::Pin,
    task::{Context, Poll},
};

use serde::Serialize;
use wasmtime::{
    Config, Result, Store,
    component::{Component, Linker, Val},
    // Only the `with_context` extension is needed; `Context` would collide
    // with `std::task::Context` below.
    error::Context as _,
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
    // Guest prints go through our own stdout sink (see `TrackedStdout`) so the
    // run loop can tell when a case's write has fully landed.
    let stdout = TrackedStdout::default();
    let mut store = Store::new(
        &engine,
        HostState {
            wasi: WasiCtxBuilder::new().stdout(stdout.clone()).build(),
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
    // fails with a missing import.
    p3::add_to_linker::<HostState>(&mut linker)?;

    let instance = linker.instantiate_async(&mut store, &component).await?;
    let count = instance.get_func(&mut store, count_test_idx).unwrap();
    let invoke = instance.get_func(&mut store, invoke_test_idx).unwrap();

    // An async-lifted export reports its result (task return) while the task
    // it ran in is still alive: a case that prints hands the task back with a
    // WAIT code and its `wasi:cli/stdout` stream write still in flight, and
    // per the world's contract the *caller* keeps polling until the task has
    // no waitables left. Driving that event loop is our job, so the whole run
    // lives inside one `run_concurrent` scope, and each test waits for the
    // stdout writes it left behind before its `end` marker is posted -
    // otherwise the prints of a case would land after the markers of the
    // cases that follow it, and the last case's prints would be lost when the
    // store is dropped.
    store
        .run_concurrent(async |accessor| -> Result<()> {
            let num_tests = {
                let mut results = [Val::Result(Ok(None))];
                count.call_concurrent(accessor, &[], &mut results).await?;
                let Val::U32(count) = results[0] else {
                    panic!();
                };
                count
            };

            for i in 0..num_tests {
                post_event(&TestEvent::TestStart { test: i });
                invoke
                    .call_concurrent(accessor, &[Val::U32(i)], &mut [])
                    .await?;
                // Wait out the stdout write the case left in flight. Every
                // round of the loop yields back to wasmtime's concurrent
                // runtime, which makes one more bit of progress on the pipe
                // carrying that write; the bytes reach our stdout before the
                // pipe is torn down, so the case's prints are out before the
                // `end` marker.
                while stdout.outstanding() > 0 {
                    tokio::task::yield_now().await;
                }
                post_event(&TestEvent::TestEnd { test: i });
            }

            Ok(())
        })
        .await?
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

/// Guest-facing `wasi:cli/stdout` that counts writes still in flight.
///
/// `invoke-test` returns at task return, and a case that printed hands back a
/// task whose `write-via-stream` pipe is still running: the host side has not
/// pumped the bytes through yet. Nothing in wasmtime's API tells the caller
/// when such a write finishes (`poll_no_interesting_tasks` only fires once the
/// guest task itself exits, which this runtime's tasks never do - they always
/// wait), so the sink keeps the count itself: one increment per stream the
/// host hands out, one per stream the host is done with. The run loop waits
/// that count out between a test's return and its `end` marker.
#[derive(Default, Clone)]
struct TrackedStdout {
    counts: Arc<StdoutWriteCounts>,
}

#[derive(Default)]
struct StdoutWriteCounts {
    started: AtomicUsize,
    finished: AtomicUsize,
}

impl TrackedStdout {
    /// Writes handed to the host that have not been fully written yet.
    ///
    /// The runtime is single-threaded (see `main`), so plain atomicity is all
    /// the synchronization these counters need.
    fn outstanding(&self) -> usize {
        let counts = &self.counts;
        counts.started.load(Ordering::Relaxed) - counts.finished.load(Ordering::Relaxed)
    }
}

impl wasmtime_wasi::cli::IsTerminal for TrackedStdout {
    fn is_terminal(&self) -> bool {
        std::io::stdout().is_terminal()
    }
}

impl wasmtime_wasi::cli::StdoutStream for TrackedStdout {
    fn async_stream(&self) -> Box<dyn tokio::io::AsyncWrite + Send + Sync> {
        self.counts.started.fetch_add(1, Ordering::Relaxed);
        Box::new(TrackedStdoutStream {
            counts: Arc::clone(&self.counts),
        })
    }
}

/// One guest stdout stream: straight through to the process's stdout, exactly
/// like wasmtime-wasi's `StdioOutputStream`, counting itself done when the
/// host drops it - which happens once the pipe has written every byte.
struct TrackedStdoutStream {
    counts: Arc<StdoutWriteCounts>,
}

impl Drop for TrackedStdoutStream {
    fn drop(&mut self) {
        self.counts.finished.fetch_add(1, Ordering::Relaxed);
    }
}

impl tokio::io::AsyncWrite for TrackedStdoutStream {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(std::io::stdout().write(buf))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(std::io::stdout().flush())
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
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
