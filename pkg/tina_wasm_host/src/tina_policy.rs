//! The `tina:policy` host import: the escalation channel
//! (`docs/capability_merger.md`). The engine calls `widen` after an
//! approval flow grants more than one call; `snapshot` reports the
//! effective document for diagnostics and parity assertions.
//!
//! Widening validates fail-closed and stays in memory for this run: the
//! engine persists grants through its own stores, and the next launch
//! recompiles the document from them. A widened directory cannot add a
//! preopen (fixed at instantiation) - it joins the policy the engine's
//! gates consult.

use wasmtime::component::Accessor;

pub mod bindings {
    wasmtime::component::bindgen!({
        path: "wit/deps/tina-policy",
        world: "tina:policy/tina-policy-host",
        imports: {
            // `widen` mutates shared policy state while the guest's other
            // calls may be parked, so the import is lowered as async with
            // store access (the same shape as `tina:process`/`tina:http`).
            "tina:policy/policy": async | store,
        },
    });
}

use bindings::tina::policy::types::{
    CapabilityDocument, CapabilityEntry, CommandRule, DirectoryAccess, DirectoryEntry,
    DomainEntry, PluginEntry, Verdict,
};

/// Maps a WIT entry to the policy's own vocabulary. Pure so tests can
/// exercise the mapping without a store.
pub(crate) fn map_entry(entry: &CapabilityEntry) -> crate::policy::CapabilityEntry {
    match entry {
        CapabilityEntry::Directory(dir) => crate::policy::CapabilityEntry::Directory {
            path: dir.path.clone(),
            access: match dir.access {
                DirectoryAccess::Read => crate::policy::DirAccess::Read,
                DirectoryAccess::Write => crate::policy::DirAccess::Write,
            },
        },
        CapabilityEntry::Domain(domain) => crate::policy::CapabilityEntry::Domain {
            domain: domain.domain.clone(),
        },
        CapabilityEntry::Command(rule) => crate::policy::CapabilityEntry::Command {
            pattern: rule.pattern.clone(),
            verdict: match rule.verdict {
                Verdict::Allow => crate::policy::Verdict::Allow,
                Verdict::Deny => crate::policy::Verdict::Deny,
                Verdict::Ask => crate::policy::Verdict::Ask,
            },
        },
        CapabilityEntry::Plugin(plugin) => crate::policy::CapabilityEntry::Plugin {
            plugin: plugin.plugin.clone(),
            grants: plugin.grants.clone(),
        },
        CapabilityEntry::Protected(path) => crate::policy::CapabilityEntry::Protected {
            path: path.clone(),
        },
    }
}

/// Projects the effective policy into the WIT document. Pure.
pub(crate) fn map_snapshot(policy: &crate::policy::CapabilityPolicy) -> CapabilityDocument {
    use crate::policy::PolicySource;
    let effective = policy.snapshot();
    let mut entries = Vec::new();
    for dir in effective.directories.write {
        entries.push(CapabilityEntry::Directory(DirectoryEntry {
            path: dir.path,
            access: DirectoryAccess::Write,
            origin: dir.origin,
        }));
    }
    for dir in effective.directories.read {
        entries.push(CapabilityEntry::Directory(DirectoryEntry {
            path: dir.path,
            access: DirectoryAccess::Read,
            origin: dir.origin,
        }));
    }
    // Legacy launches have no domain entries and an empty command table -
    // their policy lives with the guest-carried profile, so the snapshot
    // must not suggest otherwise.
    if policy.source == PolicySource::Document {
        for domain in effective.network.domains {
            entries.push(CapabilityEntry::Domain(DomainEntry {
                domain: domain.domain,
                origin: domain.origin,
            }));
        }
        for rule in effective.commands.rules {
            entries.push(CapabilityEntry::Command(CommandRule {
                pattern: rule.pattern,
                verdict: match rule.verdict.as_str() {
                    "allow" => Verdict::Allow,
                    "deny" => Verdict::Deny,
                    _ => Verdict::Ask,
                },
                origin: rule.origin,
            }));
        }
    }
    for plugin in effective.plugins.grants {
        entries.push(CapabilityEntry::Plugin(PluginEntry {
            plugin: plugin.plugin,
            grants: plugin.grants,
            origin: plugin.origin,
        }));
    }
    CapabilityDocument {
        mode: effective.mode,
        entries,
        protected_paths: effective.protected_paths,
    }
}

impl bindings::tina::policy::policy::Host for crate::HostState {}

impl<T> bindings::tina::policy::policy::HostWithStore<T> for crate::HostState
where
    T: 'static + Send,
{
    async fn widen(
        store: &Accessor<T, crate::HostState>,
        entry: CapabilityEntry,
    ) -> Result<(), String> {
        let mapped = map_entry(&entry);
        store.with(|mut access| access.get().policy.widen(mapped))
    }

    async fn snapshot(
        mut store: wasmtime::component::Access<'_, T, crate::HostState>,
    ) -> CapabilityDocument {
        let state = store.get();
        map_snapshot(&state.policy)
    }
}

/// Adds the `tina:policy/policy@1.0.0` instance to `linker`. Components
/// that do not import the interface never reach it.
pub fn add_to_linker(
    linker: &mut wasmtime::component::Linker<crate::HostState>,
) -> wasmtime::Result<()> {
    bindings::tina::policy::policy::add_to_linker::<crate::HostState, crate::HostState>(
        linker,
        |state| state,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document_policy() -> crate::policy::CapabilityPolicy {
        let ws = std::env::temp_dir().join(format!("tina-policy-wid-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();
        let canonical = std::fs::canonicalize(&ws).unwrap().display().to_string();
        let json = format!(
            r#"{{
                "version": 1,
                "mode": "auto",
                "workspace": "{canonical}",
                "directories": {{
                    "read": [],
                    "write": [{{"path": "{canonical}", "guest": ".", "origin": "structural:workspace"}}]
                }},
                "network": {{
                    "allow_all": false,
                    "domains": [{{"domain": "api.example.com", "origin": "provider:test"}}]
                }},
                "commands": {{
                    "default_verdict": "ask",
                    "rules": [{{"pattern": "git status", "verdict": "allow", "origin": "grant:g1"}}]
                }},
                "plugins": {{"grants": []}},
                "protected_paths": [],
                "preopens": [{{"guest": ".", "host": "{canonical}", "access": "rw", "origin": "structural:workspace"}}]
            }}"#
        );
        crate::policy::CapabilityPolicy::from_document_json(&json).expect("valid document")
    }

    #[test]
    fn widening_entries_map_and_validate() {
        let mut policy = document_policy();

        let before = policy.snapshot().commands.rules.len();
        policy
            .widen(map_entry(&CapabilityEntry::Command(CommandRule {
                pattern: "cargo .*".to_string(),
                verdict: Verdict::Allow,
                origin: "escalation".to_string(),
            })))
            .expect("a valid pattern widens");
        assert_eq!(policy.snapshot().commands.rules.len(), before + 1);

        // An invalid pattern is refused, not silently dropped.
        assert!(policy
            .widen(map_entry(&CapabilityEntry::Command(CommandRule {
                pattern: "[unclosed".to_string(),
                verdict: Verdict::Allow,
                origin: "escalation".to_string(),
            })))
            .is_err());

        // A domain entry joins the allowlist.
        policy
            .widen(map_entry(&CapabilityEntry::Domain(DomainEntry {
                domain: "telemetry.example.net".to_string(),
                origin: "escalation".to_string(),
            })))
            .expect("a valid domain widens");
        assert!(policy.domain_allowed("https", "telemetry.example.net", 443));
    }

    #[test]
    fn snapshot_projects_the_effective_document() {
        let mut policy = document_policy();
        policy
            .widen(map_entry(&CapabilityEntry::Domain(DomainEntry {
                domain: "widened.example.org".to_string(),
                origin: "escalation".to_string(),
            })))
            .expect("widen");

        let document = map_snapshot(&policy);
        assert_eq!(document.mode, "auto");
        assert_eq!(document.protected_paths, Vec::<String>::new());

        let domains: Vec<&String> = document
            .entries
            .iter()
            .filter_map(|entry| match entry {
                CapabilityEntry::Domain(domain) => Some(&domain.domain),
                _ => None,
            })
            .collect();
        assert_eq!(domains.len(), 2);
        assert!(domains.iter().any(|d| d.as_str() == "widened.example.org"));

        let commands = document
            .entries
            .iter()
            .filter(|entry| matches!(entry, CapabilityEntry::Command(_)))
            .count();
        assert_eq!(commands, 1);
    }

    #[test]
    fn legacy_snapshot_reports_no_command_or_domain_entries() {
        let policy = crate::policy::CapabilityPolicy::from_legacy(
            vec![(".".to_string(), std::path::PathBuf::from("/tmp/ws"))],
            vec![(
                "tina/wasm-prompt".to_string(),
                vec!["session.read-context".to_string()],
            )],
        );
        let document = map_snapshot(&policy);
        assert_eq!(document.mode, "legacy");
        assert!(document
            .entries
            .iter()
            .all(|entry| matches!(entry, CapabilityEntry::Directory(_) | CapabilityEntry::Plugin(_))));
    }
}
