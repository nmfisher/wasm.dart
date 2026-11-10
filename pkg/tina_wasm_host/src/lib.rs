//! Restricted, per-Store plugin execution boundary, plus the guest
//! runner shared by the two binaries: the packaged `tina-wasm-host`
//! (embedded engine, default discovery) and the case suite's
//! `tina-case-runner` (one positional component under the
//! `wasmdart:tests` case protocol).
pub mod plugin_loader;
pub mod policy;
pub mod runner;
pub mod tina_http;
pub mod tina_policy;
pub mod tina_plugin;
pub mod tina_process;

// The mediators hang their implementations off the concrete store
// state; keep the historical path name working.
pub use runner::HostState;
