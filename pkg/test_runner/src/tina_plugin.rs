//! Host adapter between Tina's asynchronous AgentPlugin import and the
//! per-plugin-Store boundary. The only service exposed in this probe is a
//! read of the immutable call frame captured by Tina's PluginSession.
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use wasmtime::{Engine, Result};

use test_runner::plugin_loader::{
    PluginBoundary, PluginCallFrame, PluginFailure, PluginId, PluginPolicy, PluginServiceBroker,
    ServiceCallError, ServiceCaller,
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

#[derive(Debug)]
pub enum TinaPluginError {
    Denied,
    Unavailable,
    Failed,
}

/// A single host-approved component artifact. Each `open` creates a fresh
/// plugin instance/Store in the shared boundary runtime and returns a host-issued lease string. Guest
/// IDs, descriptor text and request fields never select artifacts or grants.
pub struct TinaPluginRuntime {
    boundary: PluginBoundary,
    bytes: Vec<u8>,
    policy: PluginPolicy,
    leases: HashMap<String, PluginId>,
    next_lease: u64,
    runtime_id: u64,
}

impl TinaPluginRuntime {
    pub fn new(engine: Engine, bytes: Vec<u8>, allow_read_context: bool) -> Result<Self> {
        let runtime_id = NEXT_RUNTIME_ID
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| wasmtime::Error::msg("Tina plugin runtime identity exhausted"))?;
        Ok(Self {
            boundary: PluginBoundary::new(
                engine,
                Arc::new(TinaSessionBroker::new(allow_read_context)),
            )?,
            bytes,
            policy: PluginPolicy {
                allow_engine_service_import: true,
                allow_engine_service_call: allow_read_context,
            },
            leases: HashMap::new(),
            next_lease: 1,
            runtime_id,
        })
    }

    pub fn open(&mut self) -> std::result::Result<String, TinaPluginError> {
        let next = self
            .next_lease
            .checked_add(1)
            .ok_or_else(|| TinaPluginError::Failed)?;
        let handle = self
            .boundary
            .load(&self.bytes, self.policy)
            .map_err(|_| TinaPluginError::Failed)?;
        if let Err(_error) = self.boundary.describe(handle) {
            let _ = self.boundary.retire(handle);
            return Err(TinaPluginError::Failed);
        }
        let lease = format!("tina-plugin-lease-{}-{}", self.runtime_id, self.next_lease);
        self.next_lease = next;
        self.leases.insert(lease.clone(), handle);
        Ok(lease)
    }

    pub fn on_prompt(
        &mut self,
        lease: &str,
        frame: PluginCallFrame,
    ) -> std::result::Result<String, TinaPluginError> {
        let handle = self
            .leases
            .get(lease)
            .copied()
            .ok_or(TinaPluginError::Denied)?;
        let result = self.boundary.run_with_frame(handle, "prompt", Some(frame));
        let result = match result {
            Ok(result) => result,
            Err(_error) => {
                // A Wasmtime trap retires the Store inside PluginBoundary. Revoke
                // the host lease too, so later calls fail before touching it.
                self.leases.remove(lease);
                let _ = self.boundary.retire(handle);
                return Err(TinaPluginError::Failed);
            }
        };
        match result {
            Ok(section) => Ok(section),
            Err(PluginFailure::ServiceDenied) => Err(TinaPluginError::Denied),
            Err(PluginFailure::ServiceUnavailable) => Err(TinaPluginError::Unavailable),
            Err(PluginFailure::Rejected(_) | PluginFailure::Failed(_)) => {
                Err(TinaPluginError::Failed)
            }
        }
    }

    pub fn close(&mut self, lease: &str) -> std::result::Result<(), TinaPluginError> {
        let handle = self.leases.remove(lease).ok_or(TinaPluginError::Denied)?;
        self.boundary
            .retire(handle)
            .map_err(|_| TinaPluginError::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime::Config;

    fn engine() -> Engine {
        let mut config = Config::default();
        config.wasm_gc(true);
        config.wasm_function_references(true);
        config.wasm_exceptions(true);
        Engine::new(&config).unwrap()
    }

    fn component() -> Vec<u8> {
        include_bytes!("../testdata/plugin_probe/tina_prompt_plugin.component.wasm").to_vec()
    }

    fn frame(session_id: &str, model: &str) -> PluginCallFrame {
        PluginCallFrame {
            session_id: session_id.into(),
            model: model.into(),
        }
    }

    #[test]
    fn actual_dart_plugin_reads_call_frame_and_lease_is_revoked_on_reload() {
        let engine = engine();
        let bytes = component();
        let mut first_runtime =
            TinaPluginRuntime::new(engine.clone(), bytes.clone(), true).unwrap();
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

        let mut other_runtime = TinaPluginRuntime::new(engine, bytes, true).unwrap();
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
        let mut runtime = TinaPluginRuntime::new(engine(), component(), false).unwrap();
        let lease = runtime.open().unwrap();
        assert!(matches!(
            runtime.on_prompt(&lease, frame("denied-session", "model-a")),
            Err(TinaPluginError::Denied)
        ));
        runtime.close(&lease).unwrap();
    }
}
