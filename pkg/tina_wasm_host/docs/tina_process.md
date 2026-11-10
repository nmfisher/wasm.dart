# The `tina:process` host interface — spec

WASI has no process spawn. When the tina engine runs as a wasm component,
the HOST executes real processes on the engine's behalf: the engine sends
a spawn request, the host decides whether it may run, and the child's
output streams back through the same waitable-set machinery `tina:http`
bodies use. The interface is `tina:process/process@1.1.0`
(`wit/tina_process.wit`, implemented in `src/tina_process.rs`); the
Dart-side binding lives in the tina repository (`tina_wasm`).

1.1.0 (the capability merger, `docs/capability_merger.md`): the command
rules are HOST-HELD - the capability document the host was launched with
is the authority, and a request may carry an access profile only as a
fail-closed backstop assertion (deny beats ask beats allow). Verdict
semantics, reason strings, and the approval round-trip are unchanged.

Out of scope, deliberately: no shell, no pseudoterminal, no globbing, no
signals other than kill, no `setuid`, no timeouts. `argv` is a list of
literal strings; the host never parses it.

## Surface

```
spawn(req)   -> result<outcome, error>   (async)
respond(id, allow) -> result<outcome, error>   (async)
wait(id)     -> result<exit, error>      (async)
kill(id)     -> result<_, error>         (async; idempotent)
```

- `req` is `argv`, a guest-path `cwd`, extra `env` pairs, optional `stdin`
  bytes, and the `access profile` (below).
- `outcome` is one of
  - `started { id, stdout, stderr }` — the child runs; `stdout`/`stderr`
    are component-model byte streams, chunked as the pipes deliver,
    exactly like a `tina:http` body (the guest-side waitable-set join in
    `wasm_components` serves them);
  - `approval { id, command, reason }` — nothing runs; the engine must
    answer the request with `respond` (see *The approval round-trip*);
  - `denied(reason)` — refused, `reason` is for the engine to surface.
- `exit` is `code(n)` or `signal(n)`. The Dart side maps `signal(n)` to
  `-n`, matching `dart:io`'s exit code for a signal-terminated child.
- `error` is a host-side message (spawn failure: missing binary, bad
  `cwd`, ...). Policy is never an `error`: it arrives as `outcome`.

## The access profile

The policy table travels with the request — the engine holds tina_mode's
command rules and sends them per spawn — but ENFORCEMENT is host-side:
the host is the only side that can spawn, so it is the side that
evaluates, and it evaluates *before* any process exists.

A profile is an ordered list of rules; a rule is a regular expression
(the conservative subset tina_mode's rules use: no lookaround, no
backreferences, no `$`-interpolation) plus a verdict: `allow`, `deny`, or
`ask`. Evaluation:

1. The command line is `argv` joined with single spaces — the same
   string tina_mode shows and approves (an argument containing a space
   loses its boundary in the joined line; that is tina's display
   semantics too).
2. Each rule's pattern is anchored to the entire line (`^(?:pattern)$`)
   and compiled with the host's regex engine. An invalid pattern fails
   CLOSED: `denied("rule {i} ({pattern}) is not a valid pattern")`.
3. Rules are tried in order; the first whose anchored pattern matches
   decides. `allow` → the child starts. `deny` →
   `denied("rule {i} ({pattern}) denies the command")`. `ask` → the
   approval round-trip with reason `rule {i} ({pattern}) requires
   approval`.
4. No rule matches → the approval round-trip with reason
   `no rule matched the command`. (Fail toward the human, not toward
   silent refusal — tina_mode's default route for an unremembered
   command.)

The exact strings above are part of the interface: the native
`tina_io` port evaluates the same profile with the same rules and
produces the same strings, so engine behavior is identical on both sides
of the boundary (that equality is the exit test).

## The approval round-trip

Wasmtime cannot call back into a guest export while the guest is blocked
inside a host call, so `ask` does not call the engine — it *returns* to
it:

1. `spawn` answers `approval { id, command, reason }` immediately; the
   child does not exist yet.
2. The engine routes `command`/`reason` through its approval flow
   (tina_mode's decision pipeline; in production the approval plugin and
   the human, in the probe a programmed answer). The engine is free to
   take as long as it likes — the request has no timeout.
3. The engine answers `respond(id, allow)`:
   - `allow` → the host spawns now; `respond` returns `started` (or an
     `error` if the spawn itself fails at this point);
   - anything else → `denied("the approval flow denied the command")`.
4. Unanswered requests die with the instance: a pending approval holds
   only the request, never a process.

The same round-trip serves every `ask` outcome, whether it came from a
rule or from the no-match default.

## Request semantics

- `cwd` is a guest path, resolved against the runner's WASI preopens
  (`--preopen=guest=host`): `..` is refused, a relative path resolves
  against the first preopen, and a path no preopen covers fails with an
  error (`cwd {path} is outside the preopened directories`) — the same
  confinement `WasmFileSystemPort` applies. Absent `cwd` means the first
  preopen: an engine under this host has no other working directory.
- `env` pairs are merged over the host runner's environment, like
  `dart:io`'s `environment` parameter.
- `stdin`, when present, is written to the child's stdin, which is then
  closed. It is a byte vector, not a stream: meant for the small feeds
  `ProcessPort.run` passes. A child that never reads a large `stdin`
  will block — use `tina:http`-style streaming if that ever changes.
- stdout/stderr are piped; the child blocks when a pipe fills until the
  guest reads. Dropping a stream cancels its transfer, like `tina:http`.
- `wait` reaps the child (the id is then unknown); `kill` terminates it
  (SIGKILL on POSIX) and is idempotent. A started process that is never
  waited or killed is killed when the store drops — no orphans.
- Unknown ids fail with `error` messages (`unknown process id {id}`,
  `unknown approval id {id}`), never by silently succeeding.

## What the host does NOT trust

The engine supplies the profile, so the profile is data, not authority:
the host still refuses paths outside preopens, still anchors every
pattern, still fails closed on invalid patterns, and remains the only
side that can create a process. Tightening the host beyond the profile
(a runner-side deny list, for instance) needs no interface change — it
is another rule the host adds before evaluation.
