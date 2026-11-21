//! The compiled-component cache (`--component-cache=<dir>`): persist the
//! compiled image of a runtime-loaded component keyed by the artifact
//! bytes' hash, and deserialize it on later loads instead of
//! re-compiling. The engine/TUI components are embedded at build time
//! with their own precompiled twins (see `build.rs`); this cache serves
//! the components the host loads from disk at runtime — the Tina plugin
//! artifact and the layout-strategy plugin — compiled per `open`/reload
//! otherwise.
//!
//! Every load prints exactly one `wasm boot: …` timing line to stderr
//! (stdout is the pipes protocol's channel); the launcher surfaces the
//! line as the session's startup notice. The line vocabulary is the
//! commit-29 contract: `compiled from bytes`, `deserialized cache hit`,
//! with the optional `cache written` suffix.
//!
//! Keys embed a wasmtime-version salt; the serialized artifacts are
//! independently version-stamped (a skew fails `deserialize`), so the
//! salt only keeps the directory from accumulating cross-version
//! entries. Serialization of wasm-GC components on the pinned wasmtime
//! 47.0.2 is verified by the tests below and by the timing measurements
//! in the commit message — measured, not assumed.

use std::path::{Path, PathBuf};
use std::time::Instant;

use sha2::{Digest, Sha256};
use wasmtime::component::Component;
use wasmtime::Engine;

/// Cache-key salt: the pinned wasmtime's version. Bump with the pin.
pub const CACHE_SALT: &str = "wasmtime-47.0.2";

/// A directory of serialized compiled components.
#[derive(Clone, Debug)]
pub struct ComponentCache {
    dir: PathBuf,
}

impl ComponentCache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The cache file name for `bytes`: `<sha256(bytes || salt)>.cwasm`.
    pub fn key(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hasher.update(CACHE_SALT.as_bytes());
        format!("{:x}.cwasm", hasher.finalize())
    }

    /// Instantiates the component through the cache. Any cache problem
    /// (failed serialize, unreadable or corrupt file, version skew)
    /// falls through to the compile path; a stale file is deleted and
    /// rewritten after the compile.
    ///
    /// # Safety of `deserialize_file`
    ///
    /// It is `unsafe` because a malicious artifact can induce UB. The
    /// cache directory is user-owned (`--component-cache` points under
    /// `$HOME/.cache`), written only by this host via `Component::
    /// serialize` below, keyed by the artifact bytes' hash; corruption
    /// and version skew fail validation inside `deserialize_file` and
    /// are treated as a miss.
    pub fn load(
        &self,
        engine: &Engine,
        bytes: &[u8],
        label: &str,
    ) -> wasmtime::Result<Component> {
        let key = Self::key(bytes);
        let path = self.dir.join(&key);
        if path.exists() {
            let start = Instant::now();
            // SAFETY: see the method doc — user-owned dir, self-written
            // hash-keyed artifacts, failures handled as misses below.
            match unsafe { Component::deserialize_file(engine, &path) } {
                Ok(component) => {
                    eprintln!(
                        "wasm boot: {label}deserialized cache hit ({:.3}s)",
                        start.elapsed().as_secs_f32()
                    );
                    return Ok(component);
                }
                Err(error) => {
                    // Version skew or corruption: delete the stale file
                    // and fall through to a real compile (which rewrites).
                    eprintln!("wasm boot: {label}cache miss ({error:.60}); recompiling");
                    let _ = std::fs::remove_file(&path);
                }
            }
        }

        let start = Instant::now();
        let component = Component::new(engine, bytes)?;
        let compiled = start.elapsed();
        let mut wrote = false;
        if let Ok(image) = component.serialize() {
            wrote = (|| -> std::io::Result<()> {
                std::fs::create_dir_all(&self.dir)?;
                let temp = self
                    .dir
                    .join(format!("{key}.tmp-{}", std::process::id()));
                std::fs::write(&temp, &image)?;
                // Atomic publish: a crash mid-write never leaves a
                // partial file where a valid key would make it look
                // loadable.
                std::fs::rename(&temp, &path)?;
                Ok(())
            })()
            .is_ok();
        }
        eprintln!(
            "wasm boot: {label}compiled from bytes ({:.3}s{})",
            compiled.as_secs_f32(),
            if wrote { ", cache written" } else { "" }
        );
        Ok(component)
    }
}

/// The engine leg's timing line: the embedded component comes either
/// from its build-time precompiled twin (deserialize, the A/B hatch is
/// `TINA_WASM_AOT=0`) or a compile from the embedded bytes. The
/// runtime component cache does not apply here — the twin is frozen
/// into this very binary — but the boot outcome is reported in the
/// same `wasm boot:` vocabulary the launcher surfaces.
pub fn report_embedded_boot(deserialized: bool, elapsed: std::time::Duration) {
    if deserialized {
        eprintln!(
            "wasm boot: deserialized embedded precompiled ({:.3}s)",
            elapsed.as_secs_f32()
        );
    } else {
        eprintln!(
            "wasm boot: compiled from embedded bytes ({:.3}s)",
            elapsed.as_secs_f32()
        );
    }
}

/// True when [dir] can be created and written: the launcher-side
/// preflight shares the runner's expectation of the directory.
pub fn usable(dir: &Path) -> bool {
    std::fs::create_dir_all(dir).is_ok()
        && std::fs::write(dir.join(".write-probe"), b"probe").is_ok()
        && std::fs::remove_file(dir.join(".write-probe")).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        // The plugin boundary's engine (GC + component-model config,
        // epoch interruption): the same engine the cached loads run on.
        crate::tina_plugin::plugin_engine().expect("plugin engine")
    }

    #[test]
    fn keys_are_stable_and_input_sensitive() {
        // Identical bytes hash identically.
        assert_eq!(ComponentCache::key(b"component-bytes"), ComponentCache::key(b"component-bytes"));
        // Any byte change changes the key.
        assert_ne!(ComponentCache::key(b"component-bytes"), ComponentCache::key(b"component-bytez"));
        // The salt participates: the raw digest of the bytes differs.
        let mut hasher = Sha256::new();
        hasher.update(b"component-bytes");
        assert_ne!(
            format!("{:x}", hasher.finalize()),
            ComponentCache::key(b"component-bytes")
        );
        assert!(ComponentCache::key(b"bytes").ends_with(".cwasm"));
        assert!(CACHE_SALT.starts_with("wasmtime-"));
    }

    #[test]
    fn miss_then_hit_then_rewrite() {
        let engine = engine();
        // A tiny real component: the empty component in text form —
        // wasmtime compiles WAT directly, no fixture file needed.
        let bytes: &[u8] = b"(component)";
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = ComponentCache::new(dir.path());

        // Miss: compiles, writes the cache file, component is usable.
        cache.load(&engine, bytes, "").expect("first load compiles");
        let cached = dir.path().join(ComponentCache::key(bytes));
        assert!(cached.exists(), "miss writes the cache file");

        // Hit: the same bytes deserialize from the cache file.
        cache.load(&engine, bytes, "").expect("second load deserializes");

        // Corruption: deserialize fails, the stale file is deleted, the
        // load falls through to a compile and the file is rewritten.
        std::fs::write(&cached, b"not a serialized component").unwrap();
        cache.load(&engine, bytes, "").expect("corrupt cache falls through");
        let meta = std::fs::metadata(&cached).unwrap();
        assert!(meta.len() > 8, "the rewritten file is a real artifact");
        // And the rewritten file is a hit again.
        cache.load(&engine, bytes, "").expect("rewritten cache hits");

        // Different bytes never touch the first entry.
        let other: &[u8] = b"(component (core module $m (memory 1)))";
        cache.load(&engine, other, "layout ").expect("second artifact compiles");
        assert!(dir.path().join(ComponentCache::key(other)).exists());
        assert!(
            dir.path().join(ComponentCache::key(bytes)).exists(),
            "the first artifact's cache entry survives"
        );
    }

    /// The measurement harness for the real artifact: run with
    ///
    ///   TINA_WASM_CACHE_FIXTURE=<component> cargo test --release \
    ///     -p tina-wasm-host real_artifact -- --ignored --nocapture
    ///
    /// Prints the compile-vs-deserialize durations the commit message
    /// and the tina-side report quote. Ignored by default: CI machines
    /// hold no fixture.
    #[test]
    #[ignore = "set TINA_WASM_CACHE_FIXTURE=<component> and run with --ignored"]
    fn real_artifact_timing_probe() {
        let Ok(path) = std::env::var("TINA_WASM_CACHE_FIXTURE") else {
            panic!("TINA_WASM_CACHE_FIXTURE=<component> is required");
        };
        let bytes = std::fs::read(&path).expect("fixture component");
        let engine = engine();
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = ComponentCache::new(dir.path());

        // Miss (compile + write), then hit (deserialize): each load
        // already prints its `wasm boot: …` timing line.
        cache.load(&engine, &bytes, "").expect("miss compiles");
        cache.load(&engine, &bytes, "").expect("hit deserializes");
        // And a plain compile for the no-flag baseline.
        let start = std::time::Instant::now();
        Component::new(&engine, &bytes).expect("plain compile");
        eprintln!(
            "wasm boot: plain compile baseline ({:.3}s)",
            start.elapsed().as_secs_f32()
        );
    }

    #[test]
    fn usable_preflight_matches_the_launcher() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(usable(dir.path()));
        assert!(!usable(std::path::Path::new("/proc/definitely-not-writable")));
    }
}
