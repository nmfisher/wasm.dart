use std::{
    fs,
    io::Write,
    // `AsyncWrite`'s methods take `Pin<&mut Self>`.
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};

use serde::Serialize;
use serde_json::value::RawValue;
use wasmtime::{
    Config,
    Result,
    Store,
    StoreContextMut,
    WasmBacktrace,
    component::{Component, Linker, Val},
    // Only the `with_context` extension is needed; `Context` would collide
    // with `std::task::Context` below.
    error::Context as _,
};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView, p3};


use crate::policy;
use crate::tina_plugin;


/// Builds the config mount for default discovery: a fresh directory
/// whose `config` entry is a REAL regular file - the resolved contents
/// of `<home>/.tina/config` - so the mount is exactly stage-5-shaped
/// (`--preopen=config=<directory-containing-config>`, verified to boot
/// to ready on every platform). The user's real `~/.tina` tree is never
/// mounted: symlinks, special files, and the rest of the tree cannot
/// reach the guest's config reader, and the read-only requirement holds
/// by construction (the snapshot is never written). The snapshot lives
/// in the OS temp dir for the process lifetime; it is one small file.
/// Errors carry the friendly no-config message.
pub fn discover_config_mount(home: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let config_file = home.join(".tina").join("config");
    let friendly = || {
        format!(
            "no tina config found at {} - run tina once to create one, or pass --preopen=config=<dir>",
            config_file.display()
        )
    };
    let meta = std::fs::metadata(&config_file).map_err(|_| friendly())?;
    if !meta.is_file() {
        return Err(friendly());
    }
    let source = std::fs::canonicalize(&config_file)
        .map_err(|e| format!("could not resolve {}: {e}", config_file.display()))?;
    static NEXT_MOUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mount = std::env::temp_dir().join(format!(
        "tina-config-{}-{}",
        std::process::id(),
        NEXT_MOUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    // Recreate deterministically: a stale snapshot from a previous run of
    // the same pid must not shadow the current config.
    let _ = std::fs::remove_dir_all(&mount);
    std::fs::create_dir_all(&mount)
        .map_err(|e| format!("could not create the config snapshot {}: {e}", mount.display()))?;
    std::fs::copy(&source, mount.join("config"))
        .map_err(|e| format!("could not snapshot {}: {e}", source.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(
            mount.join("config"),
            std::fs::Permissions::from_mode(0o600),
        );
    }
    Ok(mount)
}

/// The session flags both entry points accept: the capability document,
/// legacy preopens/env, and the plugin surface. There is no engine
/// selection: the packaged host runs the engine compiled into it, and
/// the case runner takes its component as the one positional argument.
#[derive(Default)]
pub struct GuestFlags {
    pub preopens: Vec<(String, std::path::PathBuf)>,
    pub env: Vec<(String, String)>,
    pub no_stdin: bool,
    pub plugin_component: Option<std::path::PathBuf>,
    pub plugin_read_context: bool,
    pub plugin_watch: bool,
    pub plugin_call_timeout_ms: Option<u64>,
    pub capabilities: Option<String>,
}

/// Parses the shared session flags. `--engine=` and positional paths are
/// gone: the packaged host is embedded-only.
pub fn parse_flags(args: impl Iterator<Item = String>) -> Result<GuestFlags> {
    let mut flags = GuestFlags::default();
    for arg in args {
        if let Some(spec) = arg.strip_prefix("--preopen=") {
            let (guest, host) = spec.split_once('=').ok_or_else(|| {
                wasmtime::Error::msg(format!("--preopen expects guest_dir=host_dir, got {spec}"))
            })?;
            // Not preopened here: the policy decides the perms (legacy
            // folds are rw, documents derive rw/ro from the grants,
            // default discovery opens the config read-only).
            flags.preopens.push((guest.to_string(), host.into()));
        } else if arg == "--no-stdin" {
            flags.no_stdin = true;
        } else if let Some(spec) = arg.strip_prefix("--capabilities=") {
            wasmtime::ensure!(
                flags.capabilities.is_none(),
                "--capabilities given more than once"
            );
            flags.capabilities = Some(spec.to_string());
        } else if let Some(path) = arg.strip_prefix("--plugin-component=") {
            wasmtime::ensure!(
                flags.plugin_component.is_none(),
                "only one Tina plugin artifact is supported"
            );
            flags.plugin_component = Some(path.into());
        } else if arg == "--plugin-grant=session.read-context" {
            flags.plugin_read_context = true;
        } else if arg == "--plugin-watch" {
            flags.plugin_watch = true;
        } else if let Some(spec) = arg.strip_prefix("--plugin-call-timeout-ms=") {
            let value: u64 = spec.parse().map_err(|_| {
                wasmtime::Error::msg(format!(
                    "--plugin-call-timeout-ms expects u64, got {spec}"
                ))
            })?;
            wasmtime::ensure!(value > 0, "--plugin-call-timeout-ms must be positive");
            flags.plugin_call_timeout_ms = Some(value);
        } else if let Some(spec) = arg.strip_prefix("--env=") {
            let Some((name, value)) = spec.split_once('=') else {
                wasmtime::bail!("--env expects NAME=VALUE, got {spec}");
            };
            flags.env.push((name.to_string(), value.to_string()));
        } else if arg.starts_with("--engine=") {
            wasmtime::bail!("--engine was removed: the engine is compiled into this binary");
        } else if arg.starts_with('-') && arg.len() > 1 {
            wasmtime::bail!("Unknown argument {arg}");
        } else {
            wasmtime::bail!(
                "unexpected argument {arg}: the engine is compiled into this binary (case components belong to tina-case-runner)"
            );
        }
    }
    Ok(flags)
}

pub async fn run_guest(component_bytes: Vec<u8>, flags: GuestFlags) -> Result<()> {

    let mut config = Config::default();
    config.wasm_gc(true);
    config.wasm_function_references(true);
    config.wasm_exceptions(true);
    // The test export is an async component function: driving its task
    // (print lowers to a wasi:cli/stdout stream write, a timer lowers to a
    // wasi:clocks/monotonic-clock wait-for subtask) needs the concurrent
    // component API.
    config.wasm_component_model_async(true);
    // Timers are wait-for subtasks: without this the clock import is a
    // stub returning an error.
    config.wasm_component_model_more_async_builtins(true);

    let engine = wasmtime::Engine::new(&config)?;
    // Guest prints go through our own stdout sink (see `TrackedStdout`) so the
    // run loop can tell when a case's write has fully landed.
    let stdout = TrackedStdout::default();

    // Optional tina engine environment, passed on the command line after the
    // component path: `--preopen=guest_dir=host_dir` exposes a host directory
    // through a WASI preopen, `--env=NAME=VALUE` sets a WASI environment
    // variable. The case suite passes neither, so its runs are unchanged.
    // The preopen map is also what `tina:process` resolves spawn working
    // directories against (see `docs/tina_process.md`).
    //
    // `--capabilities=<path>` replaces the flag surface with the engine's
    // compiled capability document (see `docs/capability_merger.md`): the
    // host validates it fail-closed and derives the preopen set from its
    // directory grants (write grants rw, read grants ro). Without it the
    // legacy flags fold into a policy that behaves exactly like the
    // pre-merger host.
    let mut wasi = WasiCtxBuilder::new();
    wasi.stdout(stdout.clone());
    // The tina engine's `--pipes` protocol reads its driving requests
    // from stdin (the case suite's components never do). Stdin is wired
    // unless `--no-stdin` is given, so a case run piped nothing stays
    // unchanged: EOF arrives immediately for components that read.
    if !flags.no_stdin {
        // `tokio::io::stdin` blocks the single-threaded runtime's event
        // loop (its reads are runtime-thread-bound), which stalls every
        // guest - even ones that never read stdin. Instead, a plain
        // thread pumps process stdin into a channel, and the guest reads
        // the channel: interactive pipes stay live, EOF arrives as EOF.
        let (tx, rx) = tokio::sync::mpsc::channel::<bytes::Bytes>(16);
        std::thread::spawn(move || {
            use std::io::Read as _;
            let mut stdin = std::io::stdin().lock();
            let mut buffer = [0u8; 8192];
            loop {
                match stdin.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let chunk = bytes::Bytes::copy_from_slice(&buffer[..n]);
                        if tx.blocking_send(chunk).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        wasi.stdin(wasmtime_wasi::cli::AsyncStdinStream::new(ChannelReader {
            receiver: rx,
        }));
    }
    // Forwarded guest environment (the launcher's --env= flags).
    for (name, value) in &flags.env {
        wasi.env(name, value);
    }

    // The capability policy this run enforces: the engine's compiled
    // document when given, otherwise yesterday's flags folded into the
    // pre-merger behavior. The WASI preopen set is derived from it: write
    // grants open rw, read grants open read-only.
    let plugin_grants: Vec<(String, Vec<String>)> = if flags.plugin_read_context {
        vec![("tina/wasm-prompt".to_string(), vec!["session.read-context".to_string()])]
    } else {
        Vec::new()
    };
    let policy = match &flags.capabilities {
        Some(path) => {
            let text = fs::read_to_string(path)
                .with_context(|| format!("Reading capability document {path}"))?;
            policy::CapabilityPolicy::from_document_json(&text)
                .map_err(wasmtime::Error::msg)?
        }
        // Legacy flags: yesterday's launch surface, byte-compatible.
        None if !flags.preopens.is_empty() => {
            policy::CapabilityPolicy::from_legacy(flags.preopens.clone(), plugin_grants)
        }
        // Default discovery: boot a working session like native tina -
        // the current directory read-write, the user's config read-only.
        None => {
            let workspace = std::env::current_dir().map_err(|e| {
                wasmtime::Error::msg(format!("could not read the current directory: {e}"))
            })?;
            let workspace = std::fs::canonicalize(&workspace)
                .map_err(|e| wasmtime::Error::msg(format!("could not resolve the workspace: {e}")))?;
            let home = std::env::var("HOME")
                .map_err(|_| wasmtime::Error::msg("no HOME set; pass --preopen=config=<dir>".to_string()))?;
            let mount = discover_config_mount(std::path::Path::new(&home))
                .map_err(wasmtime::Error::msg)?;
            policy::CapabilityPolicy::from_default_discovery(&workspace, &mount)
        }
    };
    for preopen in policy.preopens() {
        let (dir_perms, file_perms) = match preopen.access {
            policy::DirAccess::Write => {
                (wasmtime_wasi::DirPerms::all(), wasmtime_wasi::FilePerms::all())
            }
            policy::DirAccess::Read => {
                (wasmtime_wasi::DirPerms::READ, wasmtime_wasi::FilePerms::READ)
            }
        };
        wasi
            .preopened_dir(&preopen.host, &preopen.guest, dir_perms, file_perms)
            .map_err(|e| {
                wasmtime::Error::msg(format!(
                    "preopening {} as {}: {e}",
                    preopen.host.display(),
                    preopen.guest
                ))
            })?;
    }
    // Plugin admission comes from the capability entries in document mode
    // (the flag is the legacy form and was folded above); the pre-merger
    // semantics hold otherwise: import granted, call follows read-context.
    let (import_allowed, call_allowed, read_allowed) =
        policy.plugin_admission("tina/wasm-prompt", flags.plugin_read_context);

    wasmtime::ensure!(
        flags.plugin_component.is_some() || !read_allowed,
        "a plugin grant requires --plugin-component"
    );
    let plugin_runtime = if let Some(path) = flags.plugin_component.clone() {
        let bytes = fs::read(&path)
            .with_context(|| format!("Reading plugin artifact {}", path.display()))?;
        crate::plugin_loader::verify_embedder_abi(&bytes)
            .context("plugin embedder ABI preflight")?;
        wasi.env("TINA_WASM_PLUGIN_ID", "tina/wasm-prompt");
        // Plugin exports are synchronous in this ABI, and the boundary needs
        // epoch interruption for per-call deadlines. A separate engine avoids
        // calling sync Store APIs on the async-enabled Tina engine.
        let plugin_engine = tina_plugin::plugin_engine()?;
        let mut limits = crate::plugin_loader::PluginLimits::default();
        if let Some(timeout_ms) = flags.plugin_call_timeout_ms {
            limits.call_deadline = std::time::Duration::from_millis(timeout_ms);
        }
        // Prebuilt production default: bytes frozen at startup. `--plugin-watch`
        // opts into the compile-on-load dev loop (re-read per hook attach).
        // In document mode the capability entries decide interface admission
        // and call authority; the legacy constructors keep the old shape.
        let plugin_policy = crate::plugin_loader::PluginPolicy {
            allow_engine_service_import: import_allowed,
            allow_engine_service_call: call_allowed,
        };
        let runtime = if flags.plugin_watch {
            tina_plugin::TinaPluginRuntime::watching_with_policy(
                plugin_engine,
                path.clone(),
                plugin_policy,
                read_allowed,
                limits,
            )?
        } else {
            tina_plugin::TinaPluginRuntime::with_policy(
                plugin_engine,
                bytes,
                plugin_policy,
                read_allowed,
                limits,
            )?
        };
        Some(Arc::new(runtime))
    } else {
        None
    };

    let preopens: Vec<(String, std::path::PathBuf)> = policy
        .preopens()
        .into_iter()
        .map(|p| (p.guest, p.host))
        .collect();
    let mut store = Store::new(
        &engine,
        HostState {
            wasi: wasi.build(),
            table: ResourceTable::new(),
            http: reqwest::Client::new(),
            preopens,
            process: crate::tina_process::ProcessHost::default(),
            policy,
            http_approvals: crate::tina_http::HttpApprovals::default(),
        },
    );

    let component = Component::new(&engine, component_bytes)?;
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
        collector.func_wrap(
            "record-optional-string",
            |_store, params: (Option<String>,)| {
                post_event(&TestEvent::RecordedString {
                    value: params.0.unwrap_or_else(|| "<none>".to_owned()),
                });
                Ok(())
            },
        )?;
        collector.func_wrap("record-string-list", |_store, params: (Vec<String>,)| {
            post_event(&TestEvent::RecordedString {
                value: params.0.join("|"),
            });
            Ok(())
        })?;
        collector.func_wrap("record-double", |_store, params: (f64,)| {
            post_event(&TestEvent::RecordedDouble {
                value: RawValue::from_string(dart_double_json(params.0))
                    .expect("a rendered double is always valid JSON"),
            });
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
        trace.func_wrap(
            "capture-utf16",
            |store: StoreContextMut<'_, HostState>, _capacity: (u32,)| {
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
            },
        )?;
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
            |_store, params: (u8, i32, i32, String, String)| {
                let (event_type, task_id, flow_id, name, arguments_as_json) = params;
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
    // The tina engine's HTTP port, served host-side with reqwest. Only
    // components that import `tina:http/client@1.0.0` reach it.
    crate::tina_http::add_to_linker(&mut linker)?;
    // The tina engine's process port: the host runs real processes for
    // the engine behind its host-held capability rules. Only components
    // that import `tina:process/process@1.1.0` reach it.
    crate::tina_process::add_to_linker(&mut linker)?;
    // The capability escalation channel: the engine widens the live
    // policy after an approved grant. Only components that import
    // `tina:policy/policy@1.0.0` reach it.
    crate::tina_policy::add_to_linker(&mut linker)?;
    {
        use wasmtime::component::Val;
        let mut root = linker.root();
        let mut session = root.instance("tina:plugin-host/session@0.1.0")?;
        let runtime = plugin_runtime.clone();
        session.func_new("open", move |_store, _ty, params, results| {
            wasmtime::ensure!(
                params.is_empty() && results.len() == 1,
                "unexpected plugin open signature"
            );
            results[0] = match &runtime {
                Some(runtime) => match runtime.open() {
                    Ok(lease) => Val::Result(Ok(Some(Box::new(Val::String(lease))))),
                    Err(error) => plugin_error_val(error),
                },
                None => plugin_error_val(tina_plugin::TinaPluginError::Unavailable),
            };
            Ok(())
        })?;

        let runtime = plugin_runtime.clone();
        session.func_new_concurrent("on-prompt", move |_accessor, _ty, params, results| {
            let parsed = (|| -> Result<(String, String, String)> {
                wasmtime::ensure!(
                    params.len() == 3 && results.len() == 1,
                    "unexpected on-prompt signature"
                );
                let Val::String(lease) = &params[0] else {
                    wasmtime::bail!("lease is not a string")
                };
                let Val::String(session_id) = &params[1] else {
                    wasmtime::bail!("session id is not a string")
                };
                let Val::String(model) = &params[2] else {
                    wasmtime::bail!("model is not a string")
                };
                Ok((lease.clone(), session_id.clone(), model.clone()))
            })();
            let runtime = runtime.clone();
            Box::pin(async move {
                let (lease, session_id, model) = parsed?;
                let output = match runtime {
                    Some(runtime) => match tokio::task::spawn_blocking(move || {
                        runtime.on_prompt(
                            &lease,
                            crate::plugin_loader::PluginCallFrame { session_id, model },
                        )
                    })
                    .await
                    {
                        Ok(Ok(section)) => Val::Result(Ok(Some(Box::new(Val::String(section))))),
                        Ok(Err(error)) => plugin_error_val(error),
                        Err(_) => plugin_error_val(tina_plugin::TinaPluginError::Unavailable),
                    },
                    None => plugin_error_val(tina_plugin::TinaPluginError::Unavailable),
                };
                results[0] = output;
                Ok(())
            })
        })?;

        let runtime = plugin_runtime.clone();
        session.func_new("close", move |_store, _ty, params, results| {
            wasmtime::ensure!(
                params.len() == 1 && results.len() == 1,
                "unexpected plugin close signature"
            );
            let Val::String(lease) = &params[0] else {
                wasmtime::bail!("lease is not a string")
            };
            results[0] = match &runtime {
                // close() never blocks on guest code: an active call only
                // defers the Store retirement to the end of that call.
                Some(runtime) => match runtime.close(lease) {
                    Ok(()) => Val::Result(Ok(None)),
                    Err(error) => plugin_error_val(error),
                },
                None => plugin_error_val(tina_plugin::TinaPluginError::Unavailable),
            };
            Ok(())
        })?;
    }

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

fn plugin_error_val(error: tina_plugin::TinaPluginError) -> Val {
    let (tag, payload) = match error {
        tina_plugin::TinaPluginError::Denied => ("denied", None),
        tina_plugin::TinaPluginError::Unavailable | tina_plugin::TinaPluginError::Busy => {
            ("unavailable", None)
        }
        tina_plugin::TinaPluginError::Failed => ("failed", None),
    };
    Val::Result(Err(Some(Box::new(Val::Variant(tag.to_owned(), payload)))))
}

/// The state the WASI host functions use: the WASI configuration (stdout)
/// and the resource table its streams live in, plus the shared reqwest
/// client behind the `tina:http` import, the preopen map `tina:process`
/// resolves spawn working directories against, its live children, and the
/// capability policy the mediators enforce (`docs/capability_merger.md`).
pub struct HostState {
    pub wasi: WasiCtx,
    pub table: ResourceTable,
    pub http: reqwest::Client,
    pub preopens: Vec<(String, std::path::PathBuf)>,
    pub process: crate::tina_process::ProcessHost,
    pub policy: policy::CapabilityPolicy,
    /// Parked domain approvals for `tina:http`'s `respond`.
    pub http_approvals: crate::tina_http::HttpApprovals,
}

impl HostState {
    pub fn http_client(&self) -> reqwest::Client {
        self.http.clone()
    }

    /// A minimal state for `tina_process`'s unit tests: no WASI, an idle
    /// HTTP client, no preopens, and a legacy policy so the guest-carried
    /// profile stays the command authority.
    #[cfg(test)]
    pub fn for_tests() -> Self {
        HostState {
            wasi: WasiCtxBuilder::new().build(),
            table: ResourceTable::new(),
            http: reqwest::Client::new(),
            preopens: Vec::new(),
            process: crate::tina_process::ProcessHost::default(),
            policy: policy::CapabilityPolicy::from_legacy(Vec::new(), Vec::new()),
            http_approvals: crate::tina_http::HttpApprovals::default(),
        }
    }
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

/// [tokio::io::AsyncRead] over an mpsc channel a feeder thread owns:
/// each ready chunk is one read; channel close is EOF.
struct ChannelReader {
    receiver: tokio::sync::mpsc::Receiver<bytes::Bytes>,
}

impl tokio::io::AsyncRead for ChannelReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.receiver.poll_recv(cx) {
            Poll::Ready(Some(chunk)) => {
                let n = chunk.len().min(buf.remaining());
                buf.put_slice(&chunk[..n]);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(None) => Poll::Ready(Ok(())),
            Poll::Pending => Poll::Pending,
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
    RecordedDouble { value: Box<RawValue> },
    #[serde(rename = "int")]
    RecordedInt { value: i64 },
    #[serde(rename = "bool")]
    RecordedBool { value: bool },
}

/// Renders a double the way the Dart VM does, so the host's output matches
/// what the native runner writes and what the golden files hold.
///
/// The guest hands over the bits of a Dart `double`. The goldens come from
/// `run_native.dart`, which does `json.encode(value)`, and on the VM that is
/// the same text as `double.toString()`: the ECMAScript `Number::toString`
/// layout (plain notation for -6 < n <= 21) plus a trailing `.0` on integral
/// values. serde_json instead formats with its own shortest-notation rule and
/// prints `1e-6` where the VM prints `0.000001`, so the text is produced here
/// rather than by serde_json.
///
/// NaN and ±Infinity have no JSON form - the VM's `json.encode` throws
/// `JsonUnsupportedObjectError` for them - and no case records one; `null` is
/// emitted so the line stays valid JSON.
fn dart_double_json(value: f64) -> String {
    if !value.is_finite() {
        return "null".to_string();
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0".to_string()
        } else {
            "0.0".to_string()
        };
    }
    let negative = value.is_sign_negative();
    // Rust's LowerExp formatting is the shortest decimal string that
    // round-trips, in `d[.ddd]e<exp>` form: the same digits the VM picks.
    let sci = format!("{:e}", value.abs());
    let (mantissa, exponent) = sci.split_once('e').unwrap();
    let exponent: i32 = exponent.parse().unwrap();
    let digits: String = mantissa.chars().filter(|&c| c != '.').collect();
    // With value = 0.<digits> * 10^n: k is the digit count and n the decimal
    // point's position inside `digits`.
    let k = digits.len() as i32;
    let n = exponent + 1;

    let plain = if k <= n && n <= 21 {
        // Integral value: the digits, then n - k zeros, then Dart's `.0`.
        let mut s = digits.clone();
        s.extend(core::iter::repeat('0').take((n - k) as usize));
        s.push_str(".0");
        s
    } else if n > 0 && n <= 21 {
        // The point falls inside the digits: `123.456`. (n > 21 with k <= n
        // was already taken by the integral branch above, so a plain-notation
        // n can only sit inside the digits - values like `1e+23`, whose
        // shortest digits are `1` but whose point sits 24 places in, are
        // written exponentially instead.)
        let n = n as usize;
        format!("{}.{}", &digits[..n], &digits[n..])
    } else if n > -6 && n <= 0 {
        // `0.` followed by -n zeros and the digits: `0.000001`.
        let mut s = String::from("0.");
        s.extend(core::iter::repeat('0').take((-n) as usize));
        s.push_str(&digits);
        s
    } else {
        // Exponential notation: `1.5e-7`, `1e+23`.
        let exponent = n - 1;
        let sign = if exponent < 0 { '-' } else { '+' };
        if k == 1 {
            format!("{digits}e{sign}{}", exponent.abs())
        } else {
            format!("{}.{}e{sign}{}", &digits[..1], &digits[1..], exponent.abs())
        }
    };
    if negative { format!("-{plain}") } else { plain }
}

#[cfg(test)]
mod tests {
    use super::dart_double_json;

    /// Renderings taken from the Dart VM (`dart` 3.14.0-edge, `json.encode`
    /// and `toString()` agreeing for every finite value). Each entry is the
    /// VM's own output, so parsing it back must yield the original bits.
    const VM_RENDERINGS: &[&str] = &[
        "0.0",
        "-0.0",
        "1.0",
        "-1.0",
        "1.5",
        "-1.5",
        "0.5",
        "2.25",
        "123.0",
        "12.5",
        "3.141592653589793",
        "1000.0",
        "0.001",
        "0.0015",
        "-250.0",
        "5.0",
        "0.000001",
        "1e+23",
        "-1000.0",
        "123.456",
        "-0.5",
        "20000000000.0",
        "100000000000000000000.0",
        "1e+21",
        "1e+22",
        "1e-7",
        "1.5e-7",
        "10000000000000000.0",
        "1.5e+300",
        "5e-324",
        "1.7976931348623157e+308",
        "2.2250738585072014e-308",
        "0.1",
        "0.2",
        "0.3",
        "0.3333333333333333",
        "1234567890123456.0",
        "1000000000000000.0",
        "100000000000000000.0",
        "0.00001",
        "0.0001",
        "9.95e-320",
        "1e-323",
        "2.5e-323",
        "100000.0",
        "1.0000000000000002",
        "5e-7",
        "1e-20",
        "6.02e+23",
        "-1e-7",
        "-1e+21",
        "100.0",
    ];

    #[test]
    fn renders_like_the_dart_vm() {
        for expected in VM_RENDERINGS {
            let value: f64 = expected.parse().expect("VM rendering is a valid literal");
            assert_eq!(
                dart_double_json(value),
                *expected,
                "value {expected} (bits {value:?})"
            );
        }
    }

    #[test]
    fn matches_every_double_in_double_parse_golden() {
        // The 26 doubles `double_parse.golden.txt` holds, in order.
        let golden = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../wasm_components/test/cases/double_parse.golden.txt"
        ))
        .unwrap();
        let recorded: Vec<&str> = golden
            .lines()
            .filter_map(|line| {
                line.strip_prefix(r#"{"type":"double","value":"#)
                    .map(|rest| rest.strip_suffix('}').expect("line ends with }"))
            })
            .collect();
        assert_eq!(recorded.len(), 26, "the golden's doubles");
        for value in recorded {
            let parsed: f64 = value.parse().unwrap();
            assert_eq!(dart_double_json(parsed), value, "golden value {value}");
        }
    }

    #[test]
    fn non_finite_values_have_no_json_form() {
        // The VM's `json.encode` throws for these; there is no Dart text to
        // reproduce, so the host emits JSON's null.
        assert_eq!(dart_double_json(f64::NAN), "null");
        assert_eq!(dart_double_json(f64::INFINITY), "null");
        assert_eq!(dart_double_json(f64::NEG_INFINITY), "null");
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::discover_config_mount;

    fn fixture(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tina-discover-{}-{}",
            label,
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("fixture dir");
        dir
    }

    #[test]
    fn the_mount_is_a_stage5_shaped_snapshot_of_a_real_config() {
        let home = fixture("real");
        std::fs::create_dir_all(home.join(".tina")).unwrap();
        std::fs::write(home.join(".tina").join("config"), "version = 1\n").unwrap();

        let mount = discover_config_mount(&home).expect("a real config resolves");
        let mounted = std::fs::read_to_string(mount.join("config")).expect("mounted config");
        assert_eq!(mounted, "version = 1\n");
        assert!(
            std::fs::metadata(&mount).unwrap().is_dir(),
            "the mount is a directory containing a real config file"
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_symlinked_config_snapshots_the_resolved_file() {
        // The dotfile-manager layout: ~/.tina/config -> elsewhere/tina.conf.
        // The snapshot takes the RESOLVED contents, so the guest's
        // config/config lookup works without following symlinks inside
        // the mount (the macOS-class init trap).
        let home = fixture("symlink");
        let elsewhere = fixture("elsewhere");
        std::fs::create_dir_all(home.join(".tina")).unwrap();
        std::fs::write(elsewhere.join("tina.conf"), "version = 2\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            elsewhere.join("tina.conf"),
            home.join(".tina").join("config"),
        )
        .unwrap();

        let mount = discover_config_mount(&home).expect("a symlinked config resolves");
        let mounted = std::fs::read_to_string(mount.join("config")).expect("mounted config");
        assert_eq!(mounted, "version = 2\n", "the resolved contents are mounted");
        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&elsewhere).ok();
    }

    #[test]
    fn missing_config_carries_the_friendly_message() {
        let home = fixture("missing");
        let error = discover_config_mount(&home).expect_err("no config is an error");
        assert!(
            error.contains(&format!(
                "no tina config found at {}",
                home.join(".tina").join("config").display()
            )),
            "error: {error}"
        );
        assert!(error.contains("run tina once to create one"));
        assert!(error.contains("--preopen=config=<dir>"));
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_directory_where_the_config_belongs_is_not_a_config() {
        let home = fixture("dirconfig");
        std::fs::create_dir_all(home.join(".tina").join("config")).unwrap();
        let error = discover_config_mount(&home).expect_err("a directory is not a config");
        assert!(error.contains("no tina config found at"), "error: {error}");
        std::fs::remove_dir_all(&home).ok();
    }
}
