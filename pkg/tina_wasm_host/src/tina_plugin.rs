//! Host adapter between Tina's asynchronous AgentPlugin import and the
//! per-plugin-Store boundary. The only service exposed in this probe is a
//! read of the immutable call frame captured by Tina's PluginSession.
//!
//! Concurrency contract: `open` and `close` never block on plugin code. A
//! `close` that arrives while a guest call is active revokes the lease
//! immediately and defers the Store retirement to the end of that call; an
//! `open` that arrives while the boundary is busy returns `Busy` instead of
//! waiting. Guest calls themselves run under the boundary's epoch deadline.
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use wasmtime::{Engine, Result};

use crate::plugin_loader::{
    PluginBoundary, PluginCallFrame, PluginFailure, PluginId, PluginLimits, PluginPolicy,
    PluginServiceBroker, ServiceCallError, ServiceCaller, plugin_engine_config,
};

pub const SESSION_READ_CONTEXT: &str = "session.read-context";
static NEXT_RUNTIME_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
pub struct TinaSessionBroker {
    allow_read_context: bool,
}

impl TinaSessionBroker {
    pub fn new(allow_read_context: bool) -> Self {
        Self { allow_read_context }
    }
}

impl PluginServiceBroker for TinaSessionBroker {
    fn call(
        &self,
        _caller: ServiceCaller,
        request: &str,
        frame: Option<&PluginCallFrame>,
    ) -> std::result::Result<String, ServiceCallError> {
        if !self.allow_read_context || request != SESSION_READ_CONTEXT {
            return Err(ServiceCallError::Denied);
        }
        let Some(frame) = frame else {
            return Err(ServiceCallError::Unavailable);
        };
        serde_json::to_string(&serde_json::json!({
            "session_id": &frame.session_id,
            "model": &frame.model,
        }))
        .map_err(|_| ServiceCallError::Unavailable)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TinaPluginError {
    Denied,
    Unavailable,
    /// The boundary was busy running another guest call. The operation never
    /// blocked on plugin code and may be retried once the call finishes.
    Busy,
    Failed,
}

/// Lease bookkeeping, kept behind its own lock that is never held while guest
/// code runs, so lease lookup/revocation is always wait-free.
#[derive(Default)]
struct LeaseBook {
    leases: HashMap<String, PluginId>,
    next: u64,
}

/// A single host-approved component artifact. Each `open` creates a fresh
/// plugin instance/Store in the shared boundary runtime and returns a host-issued lease string. Guest
/// IDs, descriptor text and request fields never select artifacts or grants.
///
/// All state is interior-mutable and safe to share across threads: guest
/// calls hold only the boundary lock, never the lease or pending locks.
///
/// With `artifact` set (opt-in via the runner's `--plugin-watch` flag), every
/// `open` re-reads the component file, so a rebuild of the artifact is picked
/// up by the next hook attach without restarting or rebuilding the engine.
/// Without it, the startup bytes stay frozen for the session (production
/// prebuilt behavior).
pub struct TinaPluginRuntime {
    boundary: Mutex<PluginBoundary>,
    book: Mutex<LeaseBook>,
    pending_retires: Mutex<Vec<PluginId>>,
    artifact: Option<std::path::PathBuf>,
    bytes: Mutex<Vec<u8>>,
    policy: PluginPolicy,
    runtime_id: u64,
}

impl TinaPluginRuntime {
    /// Build with explicit execution/resource budgets. The engine must come
    /// from `plugin_loader::plugin_engine_config()` so epoch deadlines work.
    pub fn with_limits(
        engine: Engine,
        bytes: Vec<u8>,
        allow_read_context: bool,
        limits: PluginLimits,
    ) -> Result<Self> {
        let runtime_id = NEXT_RUNTIME_ID
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| wasmtime::Error::msg("Tina plugin runtime identity exhausted"))?;
        Ok(Self {
            boundary: Mutex::new(PluginBoundary::with_limits(
                engine,
                Arc::new(TinaSessionBroker::new(allow_read_context)),
                limits,
            )?),
            book: Mutex::new(LeaseBook {
                next: 1,
                leases: HashMap::new(),
            }),
            pending_retires: Mutex::new(Vec::new()),
            artifact: None,
            bytes: Mutex::new(bytes),
            policy: PluginPolicy {
                allow_engine_service_import: true,
                allow_engine_service_call: allow_read_context,
            },
            runtime_id,
        })
    }

    /// Like `with_limits`, but the component file is re-read on every `open`
    /// (compile-on-load dev loop; see the type doc).
    pub fn watching_artifact(
        engine: Engine,
        artifact: std::path::PathBuf,
        allow_read_context: bool,
        limits: PluginLimits,
    ) -> Result<Self> {
        let bytes = std::fs::read(&artifact)
            .map_err(|error| wasmtime::Error::msg(format!("reading plugin artifact: {error}")))?;
        let runtime = Self::with_limits(engine, bytes, allow_read_context, limits)?;
        // Safe: the only difference from `with_limits` is the artifact path.
        let runtime_id = runtime.runtime_id;
        Ok(Self {
            artifact: Some(artifact),
            runtime_id,
            ..runtime
        })
    }

    fn drain_pending(boundary: &mut PluginBoundary, pending: &Mutex<Vec<PluginId>>) {
        let mut pending = match pending.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        for handle in pending.drain(..) {
            // Best effort: the lease is already revoked, so a cleanup-check
            // failure here cannot resurrect plugin authority. retire() still
            // drops the Store and recycles the slot on failure.
            let _ = boundary.retire(handle);
        }
    }

    pub fn open(&self) -> std::result::Result<String, TinaPluginError> {
        // Non-blocking: while a guest call runs, attaching a new hook must
        // fail fast instead of waiting on plugin code.
        let Ok(mut boundary) = self.boundary.try_lock() else {
            return Err(TinaPluginError::Busy);
        };
        Self::drain_pending(&mut boundary, &self.pending_retires);
        // Compile-on-load dev loop: pick up a rebuilt artifact before loading.
        if let Some(artifact) = &self.artifact {
            let fresh = std::fs::read(artifact).map_err(|_| TinaPluginError::Failed)?;
            let mut bytes = self.bytes.lock().map_err(|_| TinaPluginError::Failed)?;
            *bytes = fresh;
        }
        let bytes = self.bytes.lock().map_err(|_| TinaPluginError::Failed)?;
        let handle = boundary
            .load(&bytes, self.policy)
            .map_err(|_| TinaPluginError::Failed)?;
        if boundary.describe(handle).is_err() {
            let _ = boundary.retire(handle);
            return Err(TinaPluginError::Failed);
        }
        let mut book = self
            .book
            .lock()
            .map_err(|_| TinaPluginError::Failed)?;
        let next = book.next.checked_add(1).ok_or(TinaPluginError::Failed)?;
        let lease = format!("tina-plugin-lease-{}-{}", self.runtime_id, next);
        book.next = next;
        book.leases.insert(lease.clone(), handle);
        Ok(lease)
    }

    pub fn on_prompt(
        &self,
        lease: &str,
        frame: PluginCallFrame,
    ) -> std::result::Result<String, TinaPluginError> {
        let handle = {
            let book = self.book.lock().map_err(|_| TinaPluginError::Failed)?;
            book.leases.get(lease).copied().ok_or(TinaPluginError::Denied)?
        };
        let mut boundary = self.boundary.lock().map_err(|_| TinaPluginError::Failed)?;
        Self::drain_pending(&mut boundary, &self.pending_retires);
        let result = boundary.run_with_frame(handle, "prompt", Some(frame));
        // Any close() that arrived while the guest was running deferred its
        // retirement to us; the call is finished now, so retire while the
        // boundary is still exclusively ours.
        Self::drain_pending(&mut boundary, &self.pending_retires);
        match result {
            Err(_error) => {
                // A Wasmtime trap (including an epoch-deadline kill) retires
                // the Store inside PluginBoundary. Revoke the host lease too,
                // so later calls fail before touching it.
                if let Ok(mut book) = self.book.lock() {
                    book.leases.remove(lease);
                }
                let _ = boundary.retire(handle);
                Err(TinaPluginError::Failed)
            }
            Ok(inner) => {
                // A close() during the call already revoked the lease: discard
                // the result rather than return output for a closed hook.
                let still_open = self
                    .book
                    .lock()
                    .map(|book| book.leases.contains_key(lease))
                    .unwrap_or(false);
                if !still_open {
                    return Err(TinaPluginError::Failed);
                }
                match inner {
                    Ok(section) => Ok(section),
                    Err(PluginFailure::ServiceDenied) => Err(TinaPluginError::Denied),
                    Err(PluginFailure::ServiceUnavailable) => Err(TinaPluginError::Unavailable),
                    Err(PluginFailure::Rejected(_) | PluginFailure::Failed(_)) => {
                        Err(TinaPluginError::Failed)
                    }
                }
            }
        }
    }

    pub fn close(&self, lease: &str) -> std::result::Result<(), TinaPluginError> {
        let handle = {
            let mut book = self.book.lock().map_err(|_| TinaPluginError::Failed)?;
            book.leases.remove(lease).ok_or(TinaPluginError::Denied)?
        };
        match self.boundary.try_lock() {
            Ok(mut boundary) => {
                Self::drain_pending(&mut boundary, &self.pending_retires);
                boundary
                    .retire(handle)
                    .map_err(|_| TinaPluginError::Failed)
            }
            Err(_) => {
                // A guest call is active: never wait on plugin code. The
                // Store is retired as soon as that call finishes, when
                // on_prompt drains this queue under the boundary lock.
                match self.pending_retires.lock() {
                    Ok(mut pending) => pending.push(handle),
                    Err(poisoned) => poisoned.into_inner().push(handle),
                }
                Ok(())
            }
        }
    }
}

/// Convenience constructor used by the runner: a plugin engine with the
/// boundary's required epoch interruption enabled.
pub fn plugin_engine() -> Result<Engine> {
    Engine::new(&plugin_engine_config())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn engine() -> Engine {
        Engine::new(&plugin_engine_config()).unwrap()
    }

    fn component() -> Vec<u8> {
        include_bytes!("../testdata/plugin_probe/tina_prompt_plugin.component.wasm").to_vec()
    }

    fn spin_component() -> Vec<u8> {
        include_bytes!("../testdata/plugin_probe/spin.component.wasm").to_vec()
    }

    fn frame(session_id: &str, model: &str) -> PluginCallFrame {
        PluginCallFrame {
            session_id: session_id.into(),
            model: model.into(),
        }
    }

    fn spin_runtime(deadline: Duration) -> TinaPluginRuntime {
        let limits = PluginLimits {
            call_deadline: deadline,
            epoch_tick: Duration::from_millis(5),
            ..PluginLimits::default()
        };
        TinaPluginRuntime::with_limits(engine(), spin_component(), true, limits).unwrap()
    }

    #[test]
    fn actual_dart_plugin_reads_call_frame_and_lease_is_revoked_on_reload() {
        let engine = engine();
        let bytes = component();
        let first_runtime = TinaPluginRuntime::with_limits(engine.clone(), bytes.clone(), true, PluginLimits::default()).unwrap();
        let first_lease = first_runtime.open().unwrap();
        let section = first_runtime
            .on_prompt(&first_lease, frame("session-42", "model-a"))
            .unwrap();
        assert!(section.contains("session-42"));
        assert!(section.contains("model-a"));
        assert!(section.contains("run=1"));
        first_runtime.close(&first_lease).unwrap();
        assert!(matches!(
            first_runtime.on_prompt(&first_lease, frame("session-42", "model-a")),
            Err(TinaPluginError::Denied)
        ));

        let second_lease = first_runtime.open().unwrap();
        assert_ne!(first_lease, second_lease);
        let section = first_runtime
            .on_prompt(&second_lease, frame("session-42", "model-b"))
            .unwrap();
        assert!(section.contains("model-b"));
        assert!(section.contains("run=1"));

        let other_runtime = TinaPluginRuntime::with_limits(engine, bytes, true, PluginLimits::default()).unwrap();
        let other_lease = other_runtime.open().unwrap();
        assert_ne!(second_lease, other_lease);
        assert!(matches!(
            other_runtime.on_prompt(&second_lease, frame("session-42", "model-b")),
            Err(TinaPluginError::Denied)
        ));
        other_runtime.close(&other_lease).unwrap();
        first_runtime.close(&second_lease).unwrap();
    }

    #[test]
    fn ungranted_context_read_returns_denied_without_engine_callback() {
        let runtime = TinaPluginRuntime::with_limits(engine(), component(), false, PluginLimits::default()).unwrap();
        let lease = runtime.open().unwrap();
        assert!(matches!(
            runtime.on_prompt(&lease, frame("denied-session", "model-a")),
            Err(TinaPluginError::Denied)
        ));
        runtime.close(&lease).unwrap();
    }

    #[test]
    fn epoch_deadline_interrupts_spin_call_and_revokes_lease() {
        let runtime = spin_runtime(Duration::from_millis(200));
        let lease = runtime.open().unwrap();
        let started = Instant::now();
        let result = runtime.on_prompt(&lease, frame("spin-session", "model-a"));
        let elapsed = started.elapsed();
        assert!(matches!(result, Err(TinaPluginError::Failed)));
        assert!(
            elapsed < Duration::from_secs(5),
            "epoch kill took {elapsed:?}; deadline did not interrupt the guest"
        );
        // The killed call must have retired the Store and revoked the lease.
        assert!(matches!(
            runtime.on_prompt(&lease, frame("spin-session", "model-a")),
            Err(TinaPluginError::Denied)
        ));
        // The boundary stays usable: a fresh open loads into a fresh Store.
        let next = runtime.open().unwrap();
        runtime.close(&next).unwrap();
    }

    #[test]
    fn close_during_active_call_does_not_block_and_discards_result() {
        let runtime = spin_runtime(Duration::from_millis(200));
        let lease = runtime.open().unwrap();
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| runtime.on_prompt(&lease, frame("close-session", "model-a")));
            // Let the guest enter its spin loop before touching the lease.
            std::thread::sleep(Duration::from_millis(50));
            let started = Instant::now();
            let close_result = runtime.close(&lease);
            let close_elapsed = started.elapsed();
            assert_eq!(close_result, Ok(()));
            assert!(
                close_elapsed < Duration::from_millis(100),
                "close blocked on the active call for {close_elapsed:?}"
            );
            let outcome = worker.join().unwrap();
            assert!(
                matches!(outcome, Err(TinaPluginError::Failed)),
                "a call whose lease was closed mid-flight must not surface a result"
            );
        });
        // The deferred retirement ran with the call; the boundary is idle and
        // a fresh open succeeds.
        let next = runtime.open().unwrap();
        runtime.close(&next).unwrap();
    }

    #[test]
    fn watched_artifact_reload_picks_up_new_bytes_without_restart() {
        let dir = std::env::temp_dir().join(format!("issue85-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let artifact = dir.join("plugin.component.wasm");
        std::fs::write(&artifact, component()).unwrap();
        let runtime = {
            let limits = PluginLimits {
                call_deadline: Duration::from_millis(200),
                epoch_tick: Duration::from_millis(5),
                ..PluginLimits::default()
            };
            TinaPluginRuntime::watching_artifact(engine(), artifact.clone(), true, limits).unwrap()
        };

        let first = runtime.open().unwrap();
        let section = runtime
            .on_prompt(&first, frame("watch-session", "model-a"))
            .unwrap();
        assert!(section.contains("watch-session"));

        // Simulate a rebuild: same artifact path, new (spin) component bytes.
        // The engine is never restarted; the next attach must load the new
        // component while the previous instance keeps its Store and state.
        std::fs::write(&artifact, spin_component()).unwrap();
        let second = runtime.open().unwrap();
        assert_ne!(first, second);
        assert!(matches!(
            runtime.on_prompt(&second, frame("watch-session", "model-a")),
            Err(TinaPluginError::Failed), // epoch kill: the new artifact spins
        ));
        // The predecessor instance still runs its original component.
        let section = runtime
            .on_prompt(&first, frame("watch-session", "model-b"))
            .unwrap();
        assert!(section.contains("model-b"));

        runtime.close(&first).unwrap();
        // The epoch-killed call already revoked `second`'s lease, so closing
        // it again is refused instead of retiring a second time.
        assert_eq!(runtime.close(&second), Err(TinaPluginError::Denied));
        let _ = std::fs::remove_file(&artifact);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn dummy_stateful_plugin_proves_the_full_stateful_contract() {
        // One dummy artifact, six proof points on the current interface:
        // state held across calls, the explicitly granted engine service,
        // reload survival (predecessor Store keeps its state), an
        // epoch kill mid-call on the dummy's own kill marker, a fresh
        // reopen afterwards, and a clean unload with the boundary still
        // serving. No interface extension, no richer prompt frame.
        let dir = std::env::temp_dir()
            .join(format!("issue85-stateful-proof-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let artifact = dir.join("stateful_proof.component.wasm");
        let bytes = include_bytes!("../testdata/plugin_probe/stateful_proof.component.wasm");
        std::fs::write(&artifact, bytes).unwrap();

        let engine = engine();
        let limits = || PluginLimits {
            call_deadline: Duration::from_millis(200),
            epoch_tick: Duration::from_millis(5),
            ..PluginLimits::default()
        };

        // Explicit grant: without `allow_read_context` the guest's service
        // call is denied and the run fails closed.
        let ungranted =
            TinaPluginRuntime::with_limits(engine.clone(), bytes.to_vec(), false, limits()).unwrap();
        let lease = ungranted.open().unwrap();
        assert!(matches!(
            ungranted.on_prompt(&lease, frame("proof-session", "model-a")),
            Err(TinaPluginError::Denied)
        ));
        ungranted.close(&lease).unwrap();

        // Granted runtime in watch mode.
        let runtime =
            TinaPluginRuntime::watching_artifact(engine, artifact.clone(), true, limits()).unwrap();
        let lease1 = runtime.open().unwrap();
        let section = runtime
            .on_prompt(&lease1, frame("proof-session", "model-a"))
            .unwrap();
        assert!(section.contains("proof-session") && section.contains("model-a"));
        assert!(section.contains("run=1"));
        let section = runtime
            .on_prompt(&lease1, frame("proof-session", "model-a"))
            .unwrap();
        assert!(section.contains("run=2"), "state held across calls: {section:.80}");

        // Reload survival: rewrite the artifact (simulating a rebuild);
        // the next attach gets a fresh Store while the predecessor keeps
        // its own counter.
        std::fs::write(&artifact, bytes).unwrap();
        let lease2 = runtime.open().unwrap();
        assert_ne!(lease1, lease2);
        let section = runtime
            .on_prompt(&lease2, frame("proof-session", "model-b"))
            .unwrap();
        assert!(section.contains("run=1"), "fresh store after reload: {section:.80}");
        let section = runtime
            .on_prompt(&lease1, frame("proof-session", "model-a"))
            .unwrap();
        assert!(
            section.contains("run=3"),
            "predecessor state survived the reload: {section:.80}"
        );

        // Epoch kill mid-call on the dummy's own kill marker: the new
        // store's lease is revoked, the boundary stays usable.
        let started = Instant::now();
        assert!(matches!(
            runtime.on_prompt(&lease2, frame("proof-session", "kill-me")),
            Err(TinaPluginError::Failed)
        ));
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(5),
            "epoch kill took {elapsed:?}; deadline did not interrupt the guest"
        );
        assert_eq!(
            runtime.close(&lease2),
            Err(TinaPluginError::Denied),
            "a killed lease is already revoked and cannot be closed again"
        );

        // Fresh reopen after the kill: the counter restarts.
        let lease3 = runtime.open().unwrap();
        let section = runtime
            .on_prompt(&lease3, frame("proof-session", "model-c"))
            .unwrap();
        assert!(section.contains("run=1"), "{section:.80}");

        // Clean unload, session continues: the predecessor closes cleanly
        // and a final open/call/close cycle still works.
        runtime.close(&lease1).unwrap();
        let lease4 = runtime.open().unwrap();
        let section = runtime
            .on_prompt(&lease4, frame("proof-session", "model-a"))
            .unwrap();
        assert!(section.contains("run=1"), "{section:.80}");
        runtime.close(&lease4).unwrap();
        runtime.close(&lease3).unwrap();

        let _ = std::fs::remove_file(&artifact);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn open_during_active_call_is_busy_without_blocking() {
        let runtime = spin_runtime(Duration::from_millis(200));
        let lease = runtime.open().unwrap();
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| runtime.on_prompt(&lease, frame("busy-session", "model-a")));
            std::thread::sleep(Duration::from_millis(50));
            let started = Instant::now();
            let open_result = runtime.open();
            let open_elapsed = started.elapsed();
            assert!(matches!(open_result, Err(TinaPluginError::Busy)));
            assert!(
                open_elapsed < Duration::from_millis(100),
                "open blocked on the active call for {open_elapsed:?}"
            );
            // The in-flight call still fails on its deadline and the lease dies.
            assert!(matches!(
                worker.join().unwrap(),
                Err(TinaPluginError::Failed)
            ));
        });
        assert!(matches!(
            runtime.on_prompt(&lease, frame("busy-session", "model-a")),
            Err(TinaPluginError::Denied)
        ));
        // Once idle, open works again.
        let next = runtime.open().unwrap();
        runtime.close(&next).unwrap();
    }
}
