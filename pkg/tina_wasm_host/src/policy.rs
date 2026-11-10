//! The capability policy: one validated capability document per run, the
//! single source of truth the mediators enforce (see
//! `docs/capability_merger.md`).
//!
//! Two sources feed the policy:
//!
//! - **Document** (`--capabilities=<path>`): the engine compiled the full
//!   document (mode + tool policy + preopen structure). The host validates
//!   it fail-closed and enforces it at every mediator: command rules on
//!   `tina:process`, domain allowlist on `tina:http`, derived rw/ro preopens,
//!   plugin admission. Runtime widenings land here through `widen`.
//! - **Legacy flags** (no `--capabilities`): yesterday's launch surface
//!   (`--preopen`/`--plugin-grant`) folded into the same shape, with
//!   enforcement byte-compatible with the pre-merger host: command policy
//!   stays with the guest-carried per-request profile, domains are not
//!   restricted, preopens are all rw. `source` drives the difference; the
//!   mediators consult it.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use serde::Deserialize;

/// The verdict a command rule produces (the tina_mode vocabulary).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny,
    Ask,
}

impl Verdict {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "allow" => Ok(Verdict::Allow),
            "deny" => Ok(Verdict::Deny),
            "ask" => Ok(Verdict::Ask),
            other => Err(format!("unknown verdict {other:?}")),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Verdict::Allow => "allow",
            Verdict::Deny => "deny",
            Verdict::Ask => "ask",
        }
    }
}

/// Where a policy came from, and with it how far the host enforces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicySource {
    /// `--capabilities=<path>`: the host holds the authority.
    Document,
    /// Legacy flags only: the guest-carried profile stays the command
    /// authority and domains are unrestricted, exactly as before the merger.
    Legacy,
}

/// Directory preopen access derived from the grant class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirAccess {
    Read,
    Write,
}

impl DirAccess {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ro" => Ok(DirAccess::Read),
            "rw" => Ok(DirAccess::Write),
            other => Err(format!("unknown preopen access {other:?}")),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            DirAccess::Read => "ro",
            DirAccess::Write => "rw",
        }
    }
}

/// One directory grant: canonical host path, the guest name it is preopened
/// under (`None` for runtime widenings - preopens are fixed at
/// instantiation), and the entry's source for audit.
#[derive(Clone, Debug)]
pub struct DirEntry {
    pub path: PathBuf,
    pub guest: Option<String>,
    pub origin: String,
}

/// One allowed network domain: `host`, `host:port`, or `*.suffix`.
#[derive(Clone, Debug)]
pub struct DomainEntry {
    pub domain: String,
    pub origin: String,
}

/// One compiled command rule. Anchored like tina_mode's patterns; first
/// match decides.
#[derive(Clone, Debug)]
pub struct CommandRule {
    pub pattern: String,
    pub verdict: Verdict,
    pub origin: String,
    pub compiled: regex::Regex,
}

/// Grants for one plugin.
#[derive(Clone, Debug)]
pub struct PluginEntry {
    pub plugin: String,
    pub grants: Vec<String>,
    pub origin: String,
}

/// The grant vocabulary the host understands today. Unknown grants fail the
/// document (fail closed); new grants extend this set.
pub const PLUGIN_GRANTS: [&str; 3] = [
    "session.read-context",
    "engine-service-import",
    "engine-service-call",
];

/// The command verdict for a command line, with the contracted reason.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Decision {
    Allow,
    Denied { reason: String },
    Ask { reason: String },
}

/// One entry of a runtime widening (`tina:policy.widen`), already the host's
/// vocabulary.
pub enum CapabilityEntry {
    Directory { path: String, access: DirAccess },
    Domain { domain: String },
    Command { pattern: String, verdict: Verdict },
    Plugin { plugin: String, grants: Vec<String> },
    Protected { path: String },
}

// ---------------------------------------------------------------------------
// The document on the wire
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentWire {
    version: u32,
    mode: String,
    workspace: String,
    directories: DocumentDirectories,
    network: DocumentNetwork,
    commands: DocumentCommands,
    plugins: DocumentPlugins,
    protected_paths: Vec<String>,
    preopens: Vec<PreopenWire>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentDirectories {
    read: Vec<DirWire>,
    write: Vec<DirWire>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirWire {
    path: String,
    guest: String,
    origin: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentNetwork {
    allow_all: bool,
    domains: Vec<DomainWire>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DomainWire {
    domain: String,
    origin: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentCommands {
    default_verdict: String,
    rules: Vec<RuleWire>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleWire {
    pattern: String,
    verdict: String,
    origin: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentPlugins {
    grants: Vec<PluginWire>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginWire {
    plugin: String,
    grants: Vec<String>,
    origin: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreopenWire {
    guest: String,
    host: String,
    access: String,
    origin: String,
}

/// The snapshot projection: the effective document plus its derived
/// preopens, serialized the way the engine and tests read it.
#[derive(serde::Serialize)]
pub struct Snapshot {
    pub mode: String,
    pub workspace: String,
    pub directories: SnapshotDirectories,
    pub network: SnapshotNetwork,
    pub commands: SnapshotCommands,
    pub plugins: SnapshotPlugins,
    pub protected_paths: Vec<String>,
    pub preopens: Vec<SnapshotPreopen>,
}

#[derive(serde::Serialize)]
pub struct SnapshotDirectories {
    pub read: Vec<SnapshotDir>,
    pub write: Vec<SnapshotDir>,
}

#[derive(serde::Serialize)]
pub struct SnapshotDir {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guest: Option<String>,
    pub origin: String,
}

#[derive(serde::Serialize)]
pub struct SnapshotNetwork {
    pub allow_all: bool,
    pub domains: Vec<SnapshotDomain>,
}

#[derive(serde::Serialize)]
pub struct SnapshotDomain {
    pub domain: String,
    pub origin: String,
}

#[derive(serde::Serialize)]
pub struct SnapshotCommands {
    pub default_verdict: String,
    pub rules: Vec<SnapshotRule>,
}

#[derive(serde::Serialize)]
pub struct SnapshotRule {
    pub pattern: String,
    pub verdict: String,
    pub origin: String,
}

#[derive(serde::Serialize)]
pub struct SnapshotPlugins {
    pub grants: Vec<SnapshotPlugin>,
}

#[derive(serde::Serialize)]
pub struct SnapshotPlugin {
    pub plugin: String,
    pub grants: Vec<String>,
    pub origin: String,
}

#[derive(serde::Serialize)]
pub struct SnapshotPreopen {
    pub guest: String,
    pub host: String,
    pub access: String,
    pub origin: String,
}

// ---------------------------------------------------------------------------
// The validated policy
// ---------------------------------------------------------------------------

/// The validated, enforcement-ready capability policy.
#[derive(Debug)]
pub struct CapabilityPolicy {
    pub source: PolicySource,
    pub mode: String,
    pub workspace: String,
    read_dirs: Vec<DirEntry>,
    write_dirs: Vec<DirEntry>,
    allow_all_domains: bool,
    domains: Vec<DomainEntry>,
    default_verdict: Verdict,
    rules: Vec<CommandRule>,
    plugins: Vec<PluginEntry>,
    protected: Vec<PathBuf>,
}

impl CapabilityPolicy {
    /// Parses and validates a capability document. Any violation - unknown
    /// key, bad version, uncompilable pattern, missing directory, protected
    /// overlap - fails the whole document: the host refuses to start.
    pub fn from_document_json(json: &str) -> Result<Self, String> {
        let wire: DocumentWire = serde_json::from_str(json)
            .map_err(|e| format!("capability document is not valid JSON: {e}"))?;
        if wire.version != 1 {
            return Err(format!(
                "capability document version {} is not supported (want 1)",
                wire.version
            ));
        }
        const MODES: [&str; 5] = ["ask", "read-only", "allow-edits", "auto", "yolo"];
        if !MODES.contains(&wire.mode.as_str()) {
            return Err(format!(
                "capability document mode {:?} is not one of {MODES:?}",
                wire.mode
            ));
        }
        if !Path::new(&wire.workspace).is_absolute() {
            return Err(format!(
                "capability document workspace {:?} is not an absolute path",
                wire.workspace
            ));
        }

        let protected: Vec<PathBuf> = wire
            .protected_paths
            .iter()
            .map(|p| normalize_absolute(p, "protected_paths"))
            .collect::<Result<_, _>>()?;

        let mut read_dirs = Vec::new();
        let mut write_dirs = Vec::new();
        let mut guests = BTreeSet::new();
        for (class, wires) in [("read", &wire.directories.read), ("write", &wire.directories.write)]
        {
            for dir in wires {
                validate_guest_name(&dir.guest)?;
                if !guests.insert(dir.guest.clone()) {
                    return Err(format!("duplicate guest directory name {:?}", dir.guest));
                }
                let canonical = canonical_dir(&dir.path)
                    .map_err(|e| format!("{class} directory {}: {e}", dir.path))?;
                if let Some(hit) = protected_overlap(&canonical, &protected) {
                    return Err(format!(
                        "{class} directory {} overlaps protected path {}",
                        canonical.display(),
                        hit.display()
                    ));
                }
                let entry = DirEntry {
                    path: canonical,
                    guest: Some(dir.guest.clone()),
                    origin: dir.origin.clone(),
                };
                if class == "read" {
                    read_dirs.push(entry);
                } else {
                    write_dirs.push(entry);
                }
            }
        }

        let mut domains = Vec::new();
        for domain in &wire.network.domains {
            validate_domain(&domain.domain)?;
            domains.push(DomainEntry {
                domain: domain.domain.to_ascii_lowercase(),
                origin: domain.origin.clone(),
            });
        }

        let default_verdict = Verdict::parse(&wire.commands.default_verdict)?;
        let mut rules = Vec::new();
        for rule in &wire.commands.rules {
            rules.push(compile_rule(&rule.pattern, Verdict::parse(&rule.verdict)?, &rule.origin)?);
        }

        let mut plugins = Vec::new();
        for plugin in &wire.plugins.grants {
            validate_plugin(&plugin.plugin, &plugin.grants)?;
            plugins.push(PluginEntry {
                plugin: plugin.plugin.clone(),
                grants: plugin.grants.clone(),
                origin: plugin.origin.clone(),
            });
        }

        let policy = CapabilityPolicy {
            source: PolicySource::Document,
            mode: wire.mode,
            workspace: wire.workspace,
            read_dirs,
            write_dirs,
            allow_all_domains: wire.network.allow_all,
            domains,
            default_verdict,
            rules,
            plugins,
            protected,
        };
        // The document's own `preopens` array is a cross-check against the
        // host's one derivation rule, not an independent instruction: derive
        // and compare, refuse on any disagreement.
        let derived = policy.derive_preopens();
        let declared = wire
            .preopens
            .iter()
            .map(|p| {
                Ok(Preopen {
                    guest: p.guest.clone(),
                    host: canonical_dir(&p.host)?,
                    access: DirAccess::parse(&p.access)?,
                    origin: p.origin.clone(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        if declared != derived {
            return Err(format!(
                "capability document preopens disagree with the derivation from its directory grants: document has {:?}, grants derive {:?}",
                declared
                    .iter()
                    .map(|p| (p.guest.as_str(), p.host.display().to_string(), p.access.name()))
                    .collect::<Vec<_>>(),
                derived
                    .iter()
                    .map(|p| (p.guest.as_str(), p.host.display().to_string(), p.access.name()))
                    .collect::<Vec<_>>(),
            ));
        }
        Ok(policy)
    }

    /// Folds the legacy launch surface into a policy that behaves exactly
    /// like the pre-merger host: preopens as given (all rw, no
    /// canonicalization - the child's cwd stays byte-identical), plugin
    /// grants from flags, command policy left to the guest-carried profile,
    /// domains unrestricted.
    pub fn from_legacy(
        preopens: Vec<(String, PathBuf)>,
        plugin_grants: Vec<(String, Vec<String>)>,
    ) -> Self {
        CapabilityPolicy {
            source: PolicySource::Legacy,
            mode: "legacy".to_string(),
            workspace: String::new(),
            read_dirs: Vec::new(),
            write_dirs: preopens
                .into_iter()
                .map(|(guest, host)| DirEntry {
                    path: host,
                    guest: Some(guest),
                    origin: "flag:preopen".to_string(),
                })
                .collect(),
            allow_all_domains: true,
            domains: Vec::new(),
            default_verdict: Verdict::Ask,
            rules: Vec::new(),
            plugins: plugin_grants
                .into_iter()
                .map(|(plugin, grants)| PluginEntry {
                    plugin,
                    grants,
                    origin: "flag:plugin-grant".to_string(),
                })
                .collect(),
            protected: Vec::new(),
        }
    }

    /// The command verdict the host-held policy produces for one command
    /// line. `None` in legacy mode: the caller falls back to the
    /// guest-carried profile, exactly as before the merger.
    pub fn evaluate_command(&self, command_line: &str) -> Option<Decision> {
        if self.source == PolicySource::Legacy {
            return None;
        }
        Some(self.evaluate_rules(&self.rules, self.default_verdict, command_line))
    }

    /// First anchored match decides, no match uses the default verdict, an
    /// uncompilable pattern fails closed - the same shape the per-request
    /// profile has always had, now over host-held rules.
    pub fn evaluate_rules(
        &self,
        rules: &[CommandRule],
        default: Verdict,
        command_line: &str,
    ) -> Decision {
        for (i, rule) in rules.iter().enumerate() {
            if !rule.compiled.is_match(command_line) {
                continue;
            }
            return match rule.verdict {
                Verdict::Allow => Decision::Allow,
                Verdict::Deny => Decision::Denied {
                    reason: format!("rule {i} ({}) denies the command", rule.pattern),
                },
                Verdict::Ask => Decision::Ask {
                    reason: format!("rule {i} ({}) requires approval", rule.pattern),
                },
            };
        }
        match default {
            Verdict::Allow => Decision::Allow,
            Verdict::Deny => Decision::Denied {
                reason: "the capability policy denies unmatched commands".to_string(),
            },
            Verdict::Ask => Decision::Ask {
                reason: "no rule matched the command".to_string(),
            },
        }
    }

    /// The verdict for the fail-closed union of the host policy and a
    /// guest-carried backstop profile: deny beats ask beats allow. `None`
    /// profile means the host policy alone.
    pub fn evaluate_union(
        &self,
        host: Option<Decision>,
        guest: Option<Decision>,
    ) -> Decision {
        fn rank(d: &Decision) -> u8 {
            match d {
                Decision::Denied { .. } => 2,
                Decision::Ask { .. } => 1,
                Decision::Allow => 0,
            }
        }
        match (host, guest) {
            (Some(h), Some(g)) => {
                if rank(&h) >= rank(&g) {
                    h
                } else {
                    g
                }
            }
            (Some(h), None) => h,
            (None, Some(g)) => g,
            (None, None) => Decision::Ask {
                reason: "no rule matched the command".to_string(),
            },
        }
    }

    /// Whether an outgoing request to `scheme://host:port` is allowed.
    /// Legacy or `allow_all` documents allow everything; otherwise the
    /// entry list is the allowlist, with scheme-defaulted ports and
    /// `*.suffix` matching on label boundaries.
    pub fn domain_allowed(&self, scheme: &str, host: &str, port: u16) -> bool {
        if self.source == PolicySource::Legacy || self.allow_all_domains {
            return true;
        }
        let host = host.to_ascii_lowercase();
        let default_port = match scheme {
            "https" => 443,
            "http" => 80,
            _ => return false,
        };
        for entry in &self.domains {
            let domain = &entry.domain;
            let (pattern, entry_port) = match domain.rsplit_once(':') {
                Some((host_part, port_part)) => match port_part.parse::<u16>() {
                    Ok(p) if !host_part.is_empty() => (host_part, Some(p)),
                    _ => (domain.as_str(), None),
                },
                None => (domain.as_str(), None),
            };
            let port_ok = match entry_port {
                Some(p) => p == port,
                None => port == default_port,
            };
            if !port_ok {
                continue;
            }
            let host_ok = if let Some(suffix) = pattern.strip_prefix("*.") {
                // `*.example.com` matches `a.example.com` (and deeper), but
                // never `example.com` itself nor `xa.example.com`.
                host.ends_with(&format!(".{suffix}"))
            } else {
                host == pattern
            };
            if host_ok {
                return true;
            }
        }
        false
    }

    /// Grants for one plugin id (empty when none).
    pub fn plugin_grants(&self, plugin: &str) -> Vec<String> {
        self.plugins
            .iter()
            .find(|p| p.plugin == plugin)
            .map(|p| p.grants.clone())
            .unwrap_or_default()
    }

    /// Plugin admission derived from the capability entries: whether the
    /// guest may import the engine-service interface, call it, and read
    /// session context. Legacy source keeps the pre-merger semantics
    /// (import always granted, call and read-context follow the flag).
    pub fn plugin_admission(&self, plugin: &str, flag_read_context: bool) -> (bool, bool, bool) {
        match self.source {
            PolicySource::Legacy => (true, flag_read_context, flag_read_context),
            PolicySource::Document => {
                let grants = self.plugin_grants(plugin);
                let has = |name: &str| grants.iter().any(|g| g == name);
                (
                    has("engine-service-import"),
                    has("engine-service-call"),
                    has("session.read-context"),
                )
            }
        }
    }

    /// The preopen set the host materializes at instantiation: write grants
    /// rw, read grants ro, in document order, runtime widenings excluded
    /// (preopens are fixed at instantiation - a widened directory takes
    /// effect on the next launch).
    pub fn preopens(&self) -> Vec<Preopen> {
        if self.source == PolicySource::Legacy {
            return self
                .write_dirs
                .iter()
                .map(|d| Preopen {
                    guest: d.guest.clone().unwrap_or_default(),
                    host: d.path.clone(),
                    access: DirAccess::Write,
                    origin: d.origin.clone(),
                })
                .collect();
        }
        self.derive_preopens()
    }

    fn derive_preopens(&self) -> Vec<Preopen> {
        self.write_dirs
            .iter()
            .map(|d| (d, DirAccess::Write))
            .chain(self.read_dirs.iter().map(|d| (d, DirAccess::Read)))
            .filter_map(|(d, access)| {
                d.guest.as_ref().map(|guest| Preopen {
                    guest: guest.clone(),
                    host: d.path.clone(),
                    access,
                    origin: d.origin.clone(),
                })
            })
            .collect()
    }

    /// Widens the live policy with one validated entry. Rejections are
    /// fail-closed: bad pattern, missing directory, protected overlap,
    /// unknown plugin grant. Widened command rules append (a later rule
    /// never overrides an earlier one), domains append to the allowlist,
    /// plugin grants merge, protected paths only ever tighten.
    pub fn widen(&mut self, entry: CapabilityEntry) -> Result<(), String> {
        const ORIGIN: &str = "escalation";
        match entry {
            CapabilityEntry::Directory { path, access } => {
                let canonical = canonical_dir(&path)
                    .map_err(|e| format!("widened directory {path}: {e}"))?;
                if let Some(hit) = protected_overlap(&canonical, &self.protected) {
                    return Err(format!(
                        "widened directory {} overlaps protected path {}",
                        canonical.display(),
                        hit.display()
                    ));
                }
                let entry = DirEntry {
                    path: canonical,
                    guest: None,
                    origin: ORIGIN.to_string(),
                };
                if access == DirAccess::Read {
                    self.read_dirs.push(entry);
                } else {
                    self.write_dirs.push(entry);
                }
                Ok(())
            }
            CapabilityEntry::Domain { domain } => {
                validate_domain(&domain)?;
                self.domains.push(DomainEntry {
                    domain: domain.to_ascii_lowercase(),
                    origin: ORIGIN.to_string(),
                });
                Ok(())
            }
            CapabilityEntry::Command { pattern, verdict } => {
                let rule = compile_rule(&pattern, verdict, ORIGIN)?;
                self.rules.push(rule);
                Ok(())
            }
            CapabilityEntry::Plugin { plugin, grants } => {
                validate_plugin(&plugin, &grants)?;
                match self.plugins.iter_mut().find(|p| p.plugin == plugin) {
                    Some(existing) => {
                        for grant in grants {
                            if !existing.grants.contains(&grant) {
                                existing.grants.push(grant);
                            }
                        }
                        existing.origin = ORIGIN.to_string();
                    }
                    None => self.plugins.push(PluginEntry {
                        plugin,
                        grants,
                        origin: ORIGIN.to_string(),
                    }),
                }
                Ok(())
            }
            CapabilityEntry::Protected { path } => {
                let normalized = normalize_absolute(&path, "widened protected path")?;
                self.protected.push(normalized);
                Ok(())
            }
        }
    }

    /// The effective document for diagnostics and parity assertions:
    /// launch mode, entries (including runtime widenings), derived preopens.
    pub fn snapshot(&self) -> Snapshot {
        fn dirs(entries: &[DirEntry]) -> Vec<SnapshotDir> {
            entries
                .iter()
                .map(|d| SnapshotDir {
                    path: d.path.display().to_string(),
                    guest: d.guest.clone(),
                    origin: d.origin.clone(),
                })
                .collect()
        }
        Snapshot {
            mode: self.mode.clone(),
            workspace: self.workspace.clone(),
            directories: SnapshotDirectories {
                read: dirs(&self.read_dirs),
                write: dirs(&self.write_dirs),
            },
            network: SnapshotNetwork {
                allow_all: self.allow_all_domains,
                domains: self
                    .domains
                    .iter()
                    .map(|d| SnapshotDomain {
                        domain: d.domain.clone(),
                        origin: d.origin.clone(),
                    })
                    .collect(),
            },
            commands: SnapshotCommands {
                default_verdict: self.default_verdict.name().to_string(),
                rules: self
                    .rules
                    .iter()
                    .map(|r| SnapshotRule {
                        pattern: r.pattern.clone(),
                        verdict: r.verdict.name().to_string(),
                        origin: r.origin.clone(),
                    })
                    .collect(),
            },
            plugins: SnapshotPlugins {
                grants: self
                    .plugins
                    .iter()
                    .map(|p| SnapshotPlugin {
                        plugin: p.plugin.clone(),
                        grants: p.grants.clone(),
                        origin: p.origin.clone(),
                    })
                    .collect(),
            },
            protected_paths: self
                .protected
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            preopens: self
                .preopens()
                .iter()
                .map(|p| SnapshotPreopen {
                    guest: p.guest.clone(),
                    host: p.host.display().to_string(),
                    access: p.access.name().to_string(),
                    origin: p.origin.clone(),
                })
                .collect(),
        }
    }
}

/// One preopen the host materializes: guest name, canonical host path,
/// access class, and the entry it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preopen {
    pub guest: String,
    pub host: PathBuf,
    pub access: DirAccess,
    pub origin: String,
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

fn canonical_dir(path: &str) -> Result<PathBuf, String> {
    let raw = Path::new(path);
    if !raw.is_absolute() {
        return Err(format!("path {path:?} is not absolute"));
    }
    // Must exist: directory grants are derived from real directories, and
    // canonicalization resolves symlinks the same way the native runner
    // does before any grant lands.
    std::fs::canonicalize(raw).map_err(|e| format!("cannot canonicalize {path:?}: {e}"))
}

fn normalize_absolute(path: &str, what: &str) -> Result<PathBuf, String> {
    let raw = Path::new(path);
    if !raw.is_absolute() {
        return Err(format!("{what} {path:?} is not absolute"));
    }
    // Protected paths may be absent (a masked credential leaf); they are
    // normalized component-wise, `..` resolved, without requiring existence.
    let mut normalized = PathBuf::new();
    for component in raw.components() {
        match component {
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return Err(format!("{what} {path:?} climbs out of the root with `..`"));
                }
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    Ok(normalized)
}

/// The first protected path overlapping `dir`, in either direction (a
/// protected path under the directory, or the directory under a protected
/// path).
fn protected_overlap(dir: &Path, protected: &[PathBuf]) -> Option<PathBuf> {
    protected
        .iter()
        .find(|p| p.starts_with(dir) || dir.starts_with(p))
        .map(|p| p.clone())
}

fn validate_guest_name(guest: &str) -> Result<(), String> {
    let valid = !guest.is_empty()
        && guest.len() <= 128
        && guest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if valid {
        Ok(())
    } else {
        Err(format!("invalid guest directory name {guest:?}"))
    }
}

fn validate_domain(domain: &str) -> Result<(), String> {
    let domain = domain.to_ascii_lowercase();
    if domain.is_empty() || domain.len() > 253 {
        return Err(format!("invalid network domain {domain:?}"));
    }
    let (host, port) = match domain.rsplit_once(':') {
        Some((host, port)) => {
            let port: u16 = port
                .parse()
                .map_err(|_| format!("invalid network domain {domain:?}: bad port"))?;
            (host, Some(port))
        }
        None => (domain.as_str(), None),
    };
    if let Some(suffix) = host.strip_prefix("*.") {
        if suffix.is_empty() || !suffix.contains(|c: char| c != '.') {
            return Err(format!("invalid network domain {domain:?}: empty wildcard suffix"));
        }
    } else if host.is_empty() {
        return Err(format!("invalid network domain {domain:?}"));
    }
    if host.split('.').any(|label| label.is_empty()) {
        return Err(format!("invalid network domain {domain:?}: empty label"));
    }
    if !host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '*'))
    {
        return Err(format!("invalid network domain {domain:?}: bad character"));
    }
    let _ = port;
    Ok(())
}

fn validate_plugin(plugin: &str, grants: &[String]) -> Result<(), String> {
    if plugin.is_empty() {
        return Err("plugin id is empty".to_string());
    }
    for grant in grants {
        if !PLUGIN_GRANTS.contains(&grant.as_str()) {
            return Err(format!(
                "unknown plugin grant {grant:?} (want one of {PLUGIN_GRANTS:?})"
            ));
        }
    }
    Ok(())
}

fn compile_rule(pattern: &str, verdict: Verdict, origin: &str) -> Result<CommandRule, String> {
    if pattern.is_empty() || pattern.len() > 2048 {
        return Err(format!(
            "command rule pattern of {} bytes is outside the 1..=2048 range",
            pattern.len()
        ));
    }
    // Anchored like tina_mode's command patterns: the pattern must cover the
    // whole command line, so a `git status` rule never answers
    // `git status --porcelain`.
    let anchored = format!("^(?:{pattern})$");
    let compiled = regex::Regex::new(&anchored)
        .map_err(|e| format!("command rule {pattern:?} is not a valid pattern: {e}"))?;
    Ok(CommandRule {
        pattern: pattern.to_string(),
        verdict,
        origin: origin.to_string(),
        compiled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "tina-policy-test-{}-{}-{}",
            label,
            std::process::id(),
            n
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn canonical(path: &Path) -> String {
        std::fs::canonicalize(path)
            .expect("canonicalize")
            .display()
            .to_string()
    }

    fn document(dirs_read: &[&Path], dirs_write: &[&Path], preopens: &str) -> String {
        let read: Vec<String> = dirs_read
            .iter()
            .enumerate()
            .map(|(i, p)| {
                format!(
                    "{{\"path\": {:?}, \"guest\": \"read-{i}\", \"origin\": \"setting:test\"}}",
                    canonical(p)
                )
            })
            .collect();
        let write: Vec<String> = dirs_write
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let guest = if i == 0 { "." } else { "write-{i}" };
                format!(
                    "{{\"path\": {:?}, \"guest\": {guest:?}, \"origin\": \"structural:test\"}}",
                    canonical(p)
                )
            })
            .collect();
        format!(
            r#"{{
                "version": 1,
                "mode": "allow-edits",
                "workspace": "{}",
                "directories": {{
                    "read": [{}],
                    "write": [{}]
                }},
                "network": {{
                    "allow_all": false,
                    "domains": [{{"domain": "api.example.com", "origin": "provider:test"}}]
                }},
                "commands": {{
                    "default_verdict": "ask",
                    "rules": [
                        {{"pattern": "git status", "verdict": "allow", "origin": "grant:g1"}},
                        {{"pattern": "rm .*", "verdict": "deny", "origin": "grant:g2"}}
                    ]
                }},
                "plugins": {{
                    "grants": [{{"plugin": "tina/wasm-prompt", "grants": ["session.read-context", "engine-service-call"], "origin": "flag"}}]
                }},
                "protected_paths": [],
                "preopens": [{}]
            }}"#,
            canonical(dirs_write.first().copied().unwrap_or_else(|| Path::new("/"))),
            read.join(","),
            write.join(","),
            preopens
        )
    }

    fn derived_preopens_json(write: &[&Path], read: &[&Path]) -> String {
        let mut entries: Vec<String> = write
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let guest = if i == 0 { "." } else { &format!("write-{i}")[..] };
                format!(
                    "{{\"guest\": {guest:?}, \"host\": {:?}, \"access\": \"rw\", \"origin\": \"structural:test\"}}",
                    canonical(p)
                )
            })
            .collect();
        for (i, p) in read.iter().enumerate() {
            entries.push(format!(
                "{{\"guest\": \"read-{i}\", \"host\": {:?}, \"access\": \"ro\", \"origin\": \"setting:test\"}}",
                canonical(p)
            ));
        }
        entries.join(",")
    }

    fn policy_for(dirs_read: &[&Path], dirs_write: &[&Path]) -> CapabilityPolicy {
        let preopens = derived_preopens_json(dirs_write, dirs_read);
        CapabilityPolicy::from_document_json(&document(dirs_read, dirs_write, &preopens))
            .expect("valid document")
    }

    fn allow(rule: &CapabilityPolicy, line: &str) -> bool {
        matches!(rule.evaluate_command(line), Some(Decision::Allow))
    }

    fn ask_reason(rule: &CapabilityPolicy, line: &str) -> String {
        match rule.evaluate_command(line) {
            Some(Decision::Ask { reason }) => reason,
            other => panic!("expected Ask for {line}, got {other:?}"),
        }
    }

    fn denied_reason(rule: &CapabilityPolicy, line: &str) -> String {
        match rule.evaluate_command(line) {
            Some(Decision::Denied { reason }) => reason,
            other => panic!("expected Denied for {line}, got {other:?}"),
        }
    }

    #[test]
    fn document_parses_and_enforces() {
        let ws = temp_dir("ws");
        let extra = temp_dir("extra");
        let policy = policy_for(&[&extra], &[&ws]);

        assert_eq!(policy.source, PolicySource::Document);
        assert_eq!(policy.mode, "allow-edits");
        assert!(allow(&policy, "git status"));
        assert!(!allow(&policy, "git status --porcelain"));
        assert_eq!(
            denied_reason(&policy, "rm -rf x"),
            "rule 1 (rm .*) denies the command"
        );
        assert_eq!(
            ask_reason(&policy, "printf hi"),
            "no rule matched the command"
        );

        // Preopen derivation: workspace rw as ".", read grant ro.
        let preopens = policy.preopens();
        assert_eq!(preopens.len(), 2);
        assert_eq!(preopens[0].guest, ".");
        assert_eq!(preopens[0].access, DirAccess::Write);
        assert_eq!(preopens[0].host, std::fs::canonicalize(&ws).unwrap());
        assert_eq!(preopens[1].guest, "read-0");
        assert_eq!(preopens[1].access, DirAccess::Read);

        // Domains: provider host allowed, others not.
        assert!(policy.domain_allowed("https", "api.example.com", 443));
        assert!(!policy.domain_allowed("https", "other.example.com", 443));

        // Plugin grants.
        assert_eq!(
            policy.plugin_grants("tina/wasm-prompt"),
            vec![
                "session.read-context".to_string(),
                "engine-service-call".to_string()
            ]
        );
    }

    #[test]
    fn invalid_documents_fail_closed() {
        let ws = temp_dir("ws2");
        let extra = temp_dir("extra2");
        let preopens = derived_preopens_json(&[&ws], &[&extra]);
        let base = document(&[&extra], &[&ws], &preopens);

        let reject = |label: &str, json: String| {
            let error = CapabilityPolicy::from_document_json(&json)
                .expect_err(label)
                .to_string();
            assert!(!error.is_empty(), "{label} produced an empty error");
        };

        reject("bad version", base.replacen("\"version\": 1", "\"version\": 2", 1));
        reject(
            "unknown key",
            base.replace("\"mode\": \"allow-edits\"", "\"mode\": \"allow-edits\", \"extra\": true"),
        );
        reject(
            "bad mode",
            base.replace("\"mode\": \"allow-edits\"", "\"mode\": \"chaos\""),
        );
        reject(
            "bad guest name",
            base.replace("\"guest\": \"read-0\"", "\"guest\": \"read 0\""),
        );
        reject(
            "duplicate guest",
            base.replace("\"guest\": \"read-0\"", "\"guest\": \".\""),
        );
        reject(
            "missing directory",
            base.replace(
                &format!("{:?}", canonical(&ws)),
                &format!("{:?}", canonical(&ws).replace("ws2", "ws2-absent")),
            ),
        );
        reject(
            "invalid pattern",
            base.replace("\"pattern\": \"git status\"", "\"pattern\": \"[unclosed\""),
        );
        reject(
            "bad domain",
            base.replace(
                "\"domain\": \"api.example.com\"",
                "\"domain\": \"api..example.com\"",
            ),
        );
        reject(
            "unknown plugin grant",
            base.replace(
                "\"grants\": [\"session.read-context\", \"engine-service-call\"]",
                "\"grants\": [\"root\"]",
            ),
        );
        reject(
            "bad verdict",
            base.replace("\"verdict\": \"allow\"", "\"verdict\": \"perhaps\""),
        );
        reject(
            "non-absolute workspace",
            base.replace(
                &format!("\"workspace\": \"{}\"", canonical(&ws)),
                "\"workspace\": \"relative/ws\"",
            ),
        );
    }

    #[test]
    fn protected_overlap_fails_the_document() {
        let ws = temp_dir("ws3");
        let base = document(&[], &[&ws], &derived_preopens_json(&[&ws], &[]));

        // A protected path covering the granted directory (its parent).
        let parent = canonical(&ws)
            .rsplit_once('/')
            .map(|(dir, _)| dir.to_string())
            .expect("an absolute path has a parent");
        let covering = base.replace(
            "\"protected_paths\": []",
            &format!("\"protected_paths\": [\"{parent}\"]"),
        );
        assert!(CapabilityPolicy::from_document_json(&covering).is_err());

        // A protected path under a granted directory.
        let nested = document(&[], &[&ws], &derived_preopens_json(&[&ws], &[])).replace(
            "\"protected_paths\": []",
            &format!(
                "\"protected_paths\": [\"{}/.tina\"]",
                canonical(&ws)
            ),
        );
        assert!(CapabilityPolicy::from_document_json(&nested).is_err());
    }

    #[test]
    fn preopens_must_agree_with_the_derivation() {
        let ws = temp_dir("ws4");
        let wrong = derived_preopens_json(&[&ws], &[]).replace("\"access\": \"rw\"", "\"access\": \"ro\"");
        let json = document(&[], &[&ws], &wrong);
        let error = CapabilityPolicy::from_document_json(&json)
            .expect_err("mismatched preopens must fail");
        assert!(error.contains("disagree"), "error: {error}");

        // Absent preopens array is also a mismatch (declared empty).
        let empty = document(&[], &[&ws], "");
        assert!(CapabilityPolicy::from_document_json(&empty).is_err());
    }

    #[test]
    fn legacy_fold_behaves_like_yesterday() {
        // As-given path, no canonicalization, no existence requirement at
        // fold time (wasmtime still fails on a missing preopen at
        // instantiation, exactly as before).
        let policy = CapabilityPolicy::from_legacy(
            vec![(".".to_string(), PathBuf::from("/tmp/./ws"))],
            vec![("tina/wasm-prompt".to_string(), vec!["session.read-context".to_string()])],
        );

        assert_eq!(policy.source, PolicySource::Legacy);
        assert_eq!(
            policy.preopens(),
            vec![Preopen {
                guest: ".".to_string(),
                host: PathBuf::from("/tmp/./ws"),
                access: DirAccess::Write,
                origin: "flag:preopen".to_string(),
            }]
        );
        // Command policy stays with the guest-carried profile.
        assert_eq!(policy.evaluate_command("git status"), None);
        // Domains unrestricted.
        assert!(policy.domain_allowed("https", "anything.example.net", 8443));
        assert_eq!(
            policy.plugin_grants("tina/wasm-prompt"),
            vec!["session.read-context".to_string()]
        );
    }

    #[test]
    fn domain_matching_rules() {
        let ws = temp_dir("ws5");
        let mut policy = policy_for(&[], &[&ws]);
        policy.domains = vec![
            DomainEntry { domain: "api.example.com".into(), origin: "t".into() },
            DomainEntry { domain: "localhost:8080".into(), origin: "t".into() },
            DomainEntry { domain: "*.services.example.com".into(), origin: "t".into() },
        ];

        // Exact host, scheme-defaulted port.
        assert!(policy.domain_allowed("https", "api.example.com", 443));
        assert!(policy.domain_allowed("http", "api.example.com", 80));
        // Wrong port on a host-only entry.
        assert!(!policy.domain_allowed("https", "api.example.com", 8443));
        // Explicit-port entry: only that port.
        assert!(policy.domain_allowed("http", "localhost", 8080));
        assert!(!policy.domain_allowed("http", "localhost", 80));
        // Case-insensitive host.
        assert!(policy.domain_allowed("https", "API.EXAMPLE.COM", 443));
        // Wildcard: label boundary holds; deeper subdomains match, the
        // bare domain and lookalikes without the label boundary do not.
        assert!(policy.domain_allowed("https", "a.services.example.com", 443));
        assert!(policy.domain_allowed("https", "a.b.services.example.com", 443));
        assert!(policy.domain_allowed("https", "xa.services.example.com", 443));
        assert!(!policy.domain_allowed("https", "services.example.com", 443));
        assert!(!policy.domain_allowed("https", "xaservices.example.com", 443));
        // Unknown scheme is never allowed.
        assert!(!policy.domain_allowed("ftp", "api.example.com", 21));
    }

    #[test]
    fn widen_extends_without_overriding() {
        let ws = temp_dir("ws6");
        let granted = temp_dir("granted");
        let mut policy = policy_for(&[], &[&ws]);

        // A widened command rule appends: the earlier deny still wins, the
        // appended allow covers what no earlier rule matched.
        policy
            .widen(CapabilityEntry::Command {
                pattern: "cargo .*".to_string(),
                verdict: Verdict::Allow,
            })
            .expect("widen command");
        assert_eq!(
            ask_reason(&policy, "git status --porcelain"),
            "no rule matched the command"
        );
        assert_eq!(
            denied_reason(&policy, "rm -rf x"),
            "rule 1 (rm .*) denies the command"
        );
        assert!(allow(&policy, "cargo build"));

        // Widened domain joins the allowlist.
        assert!(!policy.domain_allowed("https", "telemetry.example.net", 443));
        policy
            .widen(CapabilityEntry::Domain {
                domain: "Telemetry.example.net".to_string(),
            })
            .expect("widen domain");
        assert!(policy.domain_allowed("https", "telemetry.example.net", 443));

        // Widened plugin grants merge, deduped.
        policy
            .widen(CapabilityEntry::Plugin {
                plugin: "tina/wasm-prompt".to_string(),
                grants: vec!["session.read-context".to_string(), "engine-service-import".to_string()],
            })
            .expect("widen plugin");
        assert_eq!(
            policy.plugin_grants("tina/wasm-prompt"),
            vec![
                "session.read-context".to_string(),
                "engine-service-call".to_string(),
                "engine-service-import".to_string()
            ]
        );

        // A widened directory needs an existing path and must not touch a
        // protected path; it joins the policy without a preopen.
        let error = policy
            .widen(CapabilityEntry::Directory {
                path: "/definitely/not/here".to_string(),
                access: DirAccess::Read,
            })
            .expect_err("missing directory must fail");
        assert!(error.contains("canonicalize"), "error: {error}");

        policy
            .widen(CapabilityEntry::Protected {
                path: canonical(&granted),
            })
            .expect("widen protected");
        let error = policy
            .widen(CapabilityEntry::Directory {
                path: canonical(&granted),
                access: DirAccess::Read,
            })
            .expect_err("protected overlap must fail");
        assert!(error.contains("overlaps"), "error: {error}");

        let elsewhere = temp_dir("elsewhere");
        policy
            .widen(CapabilityEntry::Directory {
                path: canonical(&elsewhere),
                access: DirAccess::Read,
            })
            .expect("widen directory");
        let widened = PathBuf::from(canonical(&elsewhere));
        assert!(policy.preopens().iter().all(|p| p.host != widened));
        assert_eq!(
            policy.snapshot().directories.read.last().unwrap().guest,
            None,
            "a widened directory has no preopen guest name"
        );
    }

    #[test]
    fn union_prefers_the_strongest_verdict() {
        let policy = CapabilityPolicy::from_legacy(vec![], vec![]);
        let d = || Decision::Denied { reason: "d".into() };
        let a = || Decision::Ask { reason: "a".into() };
        let y = || Decision::Allow;

        match policy.evaluate_union(Some(d()), Some(a())) {
            Decision::Denied { .. } => {}
            other => panic!("deny must beat ask, got {other:?}"),
        }
        match policy.evaluate_union(Some(a()), Some(y())) {
            Decision::Ask { .. } => {}
            other => panic!("ask must beat allow, got {other:?}"),
        }
        match policy.evaluate_union(Some(y()), Some(y())) {
            Decision::Allow => {}
            other => panic!("allow plus allow is allow, got {other:?}"),
        }
        match policy.evaluate_union(None, Some(d())) {
            Decision::Denied { .. } => {}
            other => panic!("guest-only deny stands, got {other:?}"),
        }
    }

    #[test]
    fn snapshot_reflects_the_effective_policy() {
        let ws = temp_dir("ws7");
        let extra = temp_dir("extra7");
        let mut policy = policy_for(&[&extra], &[&ws]);
        let snapshot = serde_json::to_string(&policy.snapshot()).expect("snapshot json");
        assert!(snapshot.contains("\"mode\":\"allow-edits\""));
        assert!(snapshot.contains("\"verdict\":\"allow\""));
        assert!(snapshot.contains("\"access\":\"ro\""));

        policy
            .widen(CapabilityEntry::Domain {
                domain: "widened.example.org".to_string(),
            })
            .expect("widen");
        let snapshot = serde_json::to_string(&policy.snapshot()).expect("snapshot json");
        assert!(snapshot.contains("widened.example.org"));
        assert!(snapshot.contains("\"origin\":\"escalation\""));
    }
}
