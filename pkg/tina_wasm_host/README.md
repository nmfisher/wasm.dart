A wasm world (and Wasmtime implementation of it) to test WebAssembly components and collect
outputs.

We use this to compile test cases into small components to compare them against the Dart VM.

## The `tina:http` host import

The runner also serves the `tina:http/client@1.0.0` host import (see
`wit/tina_http.wit`), implemented in `src/tina_http.rs` with `reqwest`:
one async `send` call per request, returning the response head plus the
body as a component-model byte stream and a delivery future - the same
shape `wasi:cli/stdin.read-via-stream` uses. Components that do not
import the interface never reach the implementation. This is the host
side of the tina engine's HTTP port (`tina_core.HttpClientPort`); the
Dart-side binding lives in the tina repository (`tina_wasm` package).

## The `tina:process` host import

The runner also serves `tina:process/process@1.0.0` (spec:
`docs/tina_process.md`, WIT: `wit/deps/tina-process/`, implementation:
`src/tina_process.rs`): WASI has no process spawn, so the host runs real
processes for the engine. The engine's spawn request carries an access
profile (allow/deny/ask per command pattern, mirroring tina_mode's);
the host evaluates it before any process exists, refuses denies with the
rule, and returns `ask` outcomes to the engine as an approval request it
answers with `respond`. Child stdout/stderr stream to the guest through
the same machinery `tina:http` bodies use; `wait` reaps, `kill`
terminates. Spawn working directories are guest paths resolved against
the runner's preopens (an engine under this host has no other working
directory). Components that do not import the interface never reach the
implementation.

## Runner arguments

```
tina-wasm-host file.wasm [--preopen=guest_dir=host_dir]... [--env=NAME=VALUE]...
tina-wasm-host file.wasm --capabilities=doc.json [--env=NAME=VALUE]...
```

`--preopen` exposes a host directory inside the guest as a WASI preopen
(read-write); `--env` sets a WASI environment variable.
`--capabilities` replaces the flag surface with the engine's compiled
capability document (see `docs/capability_merger.md`): the host
validates it fail-closed, derives the preopen set from its directory
grants (write grants rw, read grants ro), enforces its command rules at
`tina:process`, its network domain allowlist at `tina:http`, its plugin
grants at plugin admission, and accepts `tina:policy.widen` escalations
from the engine. Without it, the legacy flags fold into a policy with
the pre-merger behavior. The case suite
passes neither, so its runs are unchanged; the tina probes use them for
their workspace and provider configuration (credentials always arrive
through `--env` from the caller's environment, never from a file).
