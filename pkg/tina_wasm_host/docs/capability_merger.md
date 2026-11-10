# Capability merger: one capability list, host-enforced, approvals as runtime grants

Design doc for issue #85 (sandbox merger). Branches: `codex/issue85-capability-merger`
(tina, off `codex/issue85-plugin-management` @ 51cec2f) and `codex/issue85-capability-merger`
(wasm.dart, off `codex/issue85-loader-current-main` @ 5bb587b). This document is committed
before any implementation; the implementation commits follow it.

## 1. Goal

One compiled **capability document** is the single source of truth for what the engine may
do. The engine compiles it from the mode and tool policy; the wasm.dart host enforces it at
its mediators; on native the engine keeps enforcing but generates its checks from the same
document. Escalations (approvals) widen the capability at runtime through a new host
interface operation. User-facing approval UX does not change. Old config inputs keep
working unchanged.

Non-goals: no UI changes, no new approval semantics, no OS-sandbox backend changes, no
per-path WASI write mediation (impossible without re-instantiation), no child-process
sandboxing inside the wasm host (pre-existing gap, unchanged — see §7).

## 2. Current state (facts this design builds on)

Engine side (tina @ `codex/issue85-plugin-management`):

- Config: TOML `~/.tina/config`, version 1 (`apps/tina_cli/lib/src/assembly_config.dart`,
  `kTinaConfigVersion = 1`, `loadTinaConfig`/`parseTinaConfig`; saved atomically by
  `ConfigDocument`, `apps/tina_cli/lib/src/config_document.dart`). Unknown tables preserved.
- `tina_mode` is **not** a config key: `PermissionMode {ask, readOnly, allowEdits, auto,
  yolo}` (`packages/plugins/tina_mode/lib/mode.dart`), set at boot (`--yolo` /
  `TINA_ENGINE_YOLO`) and changed at runtime via `/mode`; session snapshot carries the mode.
- Directory grants are settings: `tina/tools/read_only_directories`,
  `tina/tools/write_directories`, `tina/tools/allow_reads_outside_workspace`,
  `tina/tools/home_caches` (packages/plugins/tina_tools/lib/src/read_directories.dart,
  write_directories.dart, sandbox_home.dart), additive across session/workspace/global,
  stored in `approvals/permissions.json`.
- Exact command grants: `CommandApprovalStore`, `~/.tina/approvals/commands.json`,
  `{"version": 1, "grants": [...]}`, identity sha256, anchored user-reviewed regexes
  (`command_pattern.dart`), disk reread on every lookup (live revocation).
- Mode policy table: `resolveAuthorization()` with `AccessCapability {read, write, execute,
  network, host, external}` and routes `allow / deny / human / review`
  (`tina_mode/lib/authorization.dart`).
- Network today is per-call booleans (`networkRequested`/`networkAllowed`) — there is **no
  domain concept** in config yet. The `network` access capability exists in the mode table.
- Native enforcement: `SandboxPlan` (`os_sandbox_runner.dart` — workspaceRoot, tinaDir,
  writablePaths, read/writeDirectories, isolateNetwork, protectedPaths, canRead/canWrite),
  `SandboxedFileSystem` (throws `SandboxViolation`), `CapabilityEnvelope` (pure parse layer),
  OS backends bwrap / sandbox-exec / passThrough.
- Wasm launch: `wasmEngineArgv()` (`apps/tina_cli/lib/src/engine_launcher.dart`) derives
  `--preopen=.=$workspace`, `--preopen=config=<dir>`, optional `--preopen=store=<dir>`,
  env `TINA_ENGINE_YOLO|STORE|RESUME`, provider credentials/endpoint forwarding.
- Wasm engine ports: `WasmProcessPort` (`tina_wasm/lib/src/wasm_process_port.dart`) sends a
  `ProcessAccessProfile` with every spawn; `WasiToolFileSystem`/`WasmPreopens` treat the
  preopen map as the filesystem wall; `AskingBoundary` keeps mode gates and asks portable.

Host side (wasm.dart @ `codex/issue85-loader-current-main`):

- `pkg/tina_wasm_host/src/main.rs`: flags `--engine=` / positional engine, `--preopen=
  guest=host` (**always DirPerms::all/FilePerms::all** — no read/write split), `--env=`,
  `--no-stdin`, `--plugin-component=`, `--plugin-grant=session.read-context`,
  `--plugin-watch`, `--plugin-call-timeout-ms=`. Unknown flags **bail**. `HostState {wasi,
  table, http: reqwest::Client::new(), preopens, process}`.
- `tina:process@1.0.0` (`tina_process.rs`): the **guest** ships the access profile
  (`rules: [{pattern, verdict}]`) with every spawn; host evaluates: anchored first match
  decides, no match asks, invalid pattern fails closed; `ask` parks a `PendingSpawn` and
  returns an `approval-request`; the engine answers `respond(id, allow)`. cwd resolves
  against preopens, `..` refused. There are **no host-held policy entries** and no argv
  path checks beyond cwd containment.
- `tina:http@1.0.0` (`tina_http.rs`): `send` parses method/url and goes straight to
  reqwest — **no domain checks at all**.
- Plugins: `PluginPolicy {allow_engine_service_import, allow_engine_service_call}`
  (`plugin_loader.rs`) — interface admission and call authority are separate grants.
- Backstops to keep: preopen cwd containment + `..` refusal, argv sanity, kill-on-drop,
  fail-closed pattern compilation, plugin ABI/admission validation.

## 3. The capability document

One JSON document, versioned, every entry explicit, every entry traceable to its source:

```json
{
  "version": 1,
  "mode": "allow-edits",
  "workspace": "/canonical/workspace/path",
  "directories": {
    "read":  [{"path": "/opt/custom-sdk", "guest": "read-0",  "origin": "setting:tina/tools/read_only_directories"}],
    "write": [{"path": "/ws",             "guest": ".",       "origin": "structural:workspace"}]
  },
  "network": {
    "allow_all": false,
    "domains": [{"domain": "api.anthropic.com", "origin": "provider:anthropic"}]
  },
  "commands": {
    "default_verdict": "ask",
    "rules": [{"pattern": "^git status$", "verdict": "allow", "origin": "grant:<grant-id>"}]
  },
  "plugins": {
    "grants": [{"plugin": "tina/wasm-prompt", "grants": ["session.read-context", "engine-service-call"], "origin": "flag"}]
  },
  "protected_paths": ["/home/u/.tina"],
  "preopens": [{"guest": ".", "host": "/ws", "access": "rw", "origin": "structural:workspace"}]
}
```

Schema rules:

- `version` is `1`. Unknown keys are rejected at load (fail closed), so future additions
  bump `version`.
- `directories.read` / `directories.write` are canonical host paths. Guest names follow the
  today's mapping: workspace `"."`, config `"config"`, store `"store"`, derived grants
  `read-<n>` / `write-<n>` in stable order.
- `network.domains` entries are `host`, `host:port`, or `*.suffix`. Match = exact
  host(+port) with scheme-defaulted port (https 443, http 80); `*.suffix` matches any
  label-boundary suffix. No match ⇒ deny (unless `allow_all`).
- `commands.rules` keep today's exact semantics: anchored first match decides, no match
  uses `default_verdict` (`ask`), compile happens at document validation — an invalid
  pattern now fails the whole document (stricter than today's per-command deny, and the
  engine validates patterns when compiling, so this never fires for engine-produced docs).
- `plugins.grants` are explicit entries; the host builds `PluginPolicy` from them.
- `protected_paths` are never preopened and are excluded from every directory entry
  (derivation refuses overlap, symlink-resolved, per today's rules).
- `preopens` is derived, informational, and exactly what the host materializes at
  instantiation: write-class entries ⇒ rw preopens, read-class ⇒ read-only preopens (p3
  `DirPerms`/`FilePerms` already support this), protected paths never preopened.

## 4. The compiler (engine side, one implementation)

New pure-Dart package `packages/libraries/tina_capability/` — **no `dart:io`** (wasm-safe),
so the same compiler runs in the tina CLI process for both enforcement sites.

Inputs → entries:

| Input | Entries produced |
| --- | --- |
| Mode (`PermissionMode`, §5 table) | `allow_all`, default verdicts, workspace access class |
| workspace / config / store paths (today's `wasmEngineArgv` derivation) | structural `directories` + `preopens` entries (`origin: structural:*`) |
| `tina/tools/read_only_directories`, `write_directories`, `home_caches`, `allow_reads_outside_workspace` | read/write directory entries |
| `~/.tina/approvals/commands.json` grants | `commands.rules` (pattern + verdict + `grant:` origin) |
| network access grants (`capability: network` access grants already expressible in `commands.json`) + provider endpoints from `[providers.*]` / `TINA_LLM_ENDPOINT` | `network.domains` |
| `[plugins]` config + `--plugin-grant` launch flags | `plugins.grants` |
| protected-path computation (credentials, tinaDir) | `protected_paths` |

Mode shaping (compiles `resolveAuthorization`'s table into entries; same observable
outcomes):

| Mode | workspace preopen | network | commands |
| --- | --- | --- | --- |
| yolo | rw | `allow_all: true` | `[{".*", allow}]` |
| auto | rw | provider domains + grants; rest ask | grants; no match ask |
| allow-edits | rw | provider domains + grants; rest ask | grants; no match ask |
| read-only | **ro** | grants only; rest ask | read-only-certified rules + grants; no match ask |
| ask | rw | provider domains + grants; rest ask | grants; no match ask |

Why ask-mode workspace stays rw while read-only goes ro: WASI preopens are fixed at
instantiation and cannot be widened mid-run, so an approved "Allow once" write must find
the workspace already structurally open — the per-operation gate stays engine-side, exactly
like native ask mode (OS grants the session the workspace; approval gates the operation).
Read-only hard-blocks at the host too, matching "blocked before remembered grants". This is
the deliberate "two enforcement sites" split; observable results are identical (§6).

Outputs: the in-memory `CapabilityDocument` model (native enforcement consumes it directly)
and its JSON encoding (wasm host consumes it via `--capabilities=<path>`, written by the
launcher, 0600, next to the session's config — under the config dir, never inside
`protected_paths`).

## 5. Config migration and backward compatibility

"Old format must keep working — read legacy, save new", concretely:

- **Read**: TOML config v1, `approvals/permissions.json`, and `commands.json` parse exactly
  as today. If a saved capability document exists, its entries are merged **additively**
  (a document can add entries, never silently drop legacy ones); absent document ⇒ pure
  legacy compile. Legacy-only configs must compile to a document equivalent to today's
  effective grants (golden-tested, §6).
- **Save**: dual-write. The engine keeps writing the legacy stores it writes today
  (`permissions.json` for directory grants, `commands.json` for exact grants — revocation
  and Settings → Permissions keep working unchanged) **and** refreshes the compiled
  capability document. The document is the derived new-format artifact; the legacy stores
  stay authoritative this program. Cutover (document authoritative, legacy read-only) is a
  future, separately-tested change and is explicitly out of scope.
- **Host flags**: `--preopen`, `--env`, `--plugin-grant` keep their exact behavior; a host
  started without `--capabilities` folds flag-derived entries into its policy exactly as
  today (preopen ⇒ rw directory entry, plugin grant ⇒ plugin entry). Old launchers against
  the new host therefore keep working unchanged. New launcher + old host is **not**
  supported (the old host bails on unknown flags, and the two ship as one packaged binary);
  the supported bridge direction is old inputs → new host.
- `--capabilities=<path>` is the only new host flag. When present, preopens are derived
  from the document at instantiation (same guest names as today, so guest path semantics do
  not move); when absent, flag behavior is unchanged.

Compat decision note: replacing the legacy stores as save targets was considered and
rejected — it would break live revocation (`covers()` rereads `commands.json`) and the
permissions UI, i.e. observable behavior. Dual-write is the only variant that satisfies
"identical behavior to today". Not escalated: the compat clause itself decides it.

## 6. Host side (wasm.dart)

New module `pkg/tina_wasm_host/src/policy.rs`:

- Parse/validate the document (version, unknown keys, pattern compilation, path
  canonicalization, protected overlap) → `CapabilityPolicy`. Invalid document ⇒ host
  refuses to start (fail closed).
- Fold legacy flags (`--preopen`/`--plugin-grant`) into entries when no document is given.
- Lookup APIs the mediators call: command verdict, domain verdict, preopen derivation
  (rw/ro), plugin policy, snapshot for diagnostics.
- `widen(entry)` merge: insert/replace by origin key; entry re-validated (pattern compiles,
  path canonical, not protected); errors rejected. In-memory for the run.

Mediators:

- **`tina:process` (bump to 1.1.0)**: spawn evaluates the **host-held** command rules
  (identical anchored/first-match/no-match-asks semantics). The request's `profile` field
  becomes `option<access-profile>`: `None` ⇒ host policy only; `Some(p)` ⇒ fail-closed
  union (host deny or guest-profile deny ⇒ deny; either asks ⇒ ask) — old guests and old
  tests stay valid, the guest copy is a backstop assertion, never the authority. cwd
  resolves against the derived preopen map (unchanged backstop). Deny/ask reason strings
  keep the `rule <i> (<pattern>) ...` contract where the rule came from the document.
- **`tina:http` (bump to 1.1.0)**: `send` returns an outcome variant —
  `sent(head, stream, future) | approval(approval-request) | denied(reason)` — plus
  `respond(id, allow)`, mirroring `tina:process` exactly (the host cannot call into a
  blocked guest). Domain not allowed ⇒ ask (or denied when the compiled default is deny),
  allowed ⇒ request proceeds. Method/url/header handling unchanged.
- **Preopens**: derived from the document's `preopens` entries at instantiation; write
  grants rw, read grants ro, protected paths never preopened. `WasiCtxBuilder` setup moves
  behind the policy module.
- **Plugins**: `PluginPolicy` built from `plugins.grants` entries (session.read-context,
  engine-service-import/call) — admission logic untouched, source changes.
- **Backstops kept**: preopen cwd containment + `..` refusal, argv sanity, kill-on-drop,
  fail-closed pattern compile, plugin ABI/admission validation.

New host interface `tina:policy@1.0.0` (escalation channel):

```wit
package tina:policy@1.0.0;
interface policy {
  use types.{capability-entry, capability-document};
  /// Widen the live policy after an approved escalation. Validated; rejects
  /// on invalid entries or protected overlap. In-memory for this run.
  widen: async func(entry: capability-entry) -> result<_, string>;
  /// The effective document (diagnostics + parity assertions).
  snapshot: func() -> capability-document;
}
```

`capability-entry` is the variant of single entries (directory-read / directory-write /
network-domain / command-rule / plugin-grant / protected-path). Persistence does **not**
flow through the host: the engine persists through its existing stores (§5) and the next
launch recompiles. Host policy state is per-run memory, which keeps exactly one persistence
truth.

## 7. Engine side (tina)

- **Compile**: the launcher (wasm) and the engine assembly (native) call the same
  `tina_capability` compiler; tina_mode + tool policy + preopen derivation go in, one
  document comes out (§4). The launcher writes it and passes `--capabilities=<path>`;
  native keeps it in memory.
- **Native enforcement generated from the document**: `SandboxPlan`
  (read/writeDirectories, writablePaths, isolateNetwork), `SandboxedFileSystem`
  (workspace + protected + grants walls) and the process runner's command rules are
  constructed from the compiled document instead of independently from settings. Live
  grant rereads (`commands.json`/`permissions.json`) remain authoritative for exact grants
  at decision time — revocation stays live; the document is the structural seed.
- **Wasm: stop re-enforcing what the host enforces**: `WasiToolFileSystem`/`AskingBoundary`
  keep mode gates and asks (approvals are policy, not containment) but drop duplicate
  containment checks the preopen wall already guarantees; `WasmProcessPort` stops shipping
  a per-request profile as authority (`profile: null` by default); `HttpClientPort` gains
  the outcome shape and routes domain asks through the approval channel. Mode-table denies
  (read-only mutations, unconfined execution) stay engine-side — they are policy routes,
  identical on both builds.
- **Escalation flow (wasm)**: mediator returns `approval` outcome → engine asks via the
  existing approval UX (unchanged) →
  - deny/cancel ⇒ `respond(id, false)`; nothing widened, nothing persisted;
  - allow once ⇒ `respond(id, true)`; op runs; nothing widened;
  - allow always / scope ⇒ persist via the existing store for that scope (unchanged paths,
    formats, revocation) **and** call `policy.widen(entry)` so later calls this session do
    not re-ask.
  Native escalation is today's flow unchanged (the document is already the policy it
  consults).
- **Approvals UX**: no changes — same requests, same answers, same remember scopes.

Known limitation (pre-existing, documented, unchanged by this design): a runtime-widened
directory grant to a host path that was never preopened is not guest-reachable until
relaunch (preopens are fixed at instantiation). Workspace/config/store — the structural
preopens — cover the parity scenarios. Command and domain escalations are effective
immediately on both sites.

## 8. Validation: parity matrix + backward compat

One scenario table, consumed by both runners (native: decision-pipeline tests; wasm:
`tina_wasm` tool/scenarios + host integration tests). Every row asserts the same
allow/deny route and, where contracted, the same reason string:

| # | Scenario | Assert |
| --- | --- | --- |
| 1 | write inside workspace (ask / allow-edits / read-only / yolo) | approval-then-success / success / deny / success — equal on both |
| 2 | write outside workspace | deny on both (native `SandboxPlan.canWrite`; wasm preopen wall) |
| 3 | domain allowed (provider / remembered / yolo) | request proceeds on both |
| 4 | domain denied (unknown host, ask/auto) | ask outcome on both; deny after human deny on both |
| 5 | command allowed by grant rule | spawn proceeds on both |
| 6 | command denied by deny rule | denied with the contracted reason on both |
| 7 | command with no matching rule | ask on both |
| 8 | escalation approved (allow once) | op runs; not persisted; re-ask next call — both |
| 9 | escalation approved (allow always) | op runs; `widen` applied; no re-ask this session; persisted in the legacy store — both |
| 10 | escalation denied | denied; nothing persisted; re-ask next call — both |
| 11 | always-allow persistence across restart | recompiled document grants without asking — both |
| 12 | legacy config files (TOML v1 + permissions.json + commands.json only) | compiled document ≡ today's effective grants; decisions identical to pre-merger goldens |
| 13 | host launched with legacy flags only | behavior identical to today (flags folded into entries) |

Rules: tests green before every commit; no test weakened to pass (interface-shape updates
carry equal-or-stronger assertions); branches only, no merges, no PRs; identities recorded.

## 9. Implementation order

1. tina: `tina_capability` package (model, compiler, JSON codec) + legacy-compile goldens.
2. wasm.dart: `policy.rs` (parse/validate/fold/derive) + unit tests.
3. wasm.dart: preopen derivation behind the policy module + `tina:process@1.1.0` on
   host-held policy (`profile` optional) + tests.
4. wasm.dart: `tina:http@1.1.0` outcome shape + domain checks + tests.
5. wasm.dart: `tina:policy@1.0.0` widen/snapshot + tests.
6. tina: launcher emits `--capabilities`; ports to the 1.1.0 shapes; escalation wiring
   (`widen` after remembered approvals); native enforcement constructed from the document;
   wasm containment-check dedup.
7. Parity harness (§8 matrix) + compat goldens; full suites green in both repos.
8. Push both branches; record identities + artifact shas in the replay ledger.
