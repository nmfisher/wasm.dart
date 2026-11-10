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

The packaged host is **embedded-only**: the engine component is compiled
into the binary (`TINA_WASM_HOST_ENGINE=<component>` at build time) and
there is no `--engine` flag and no positional engine path — engine
updates require rebuilding the host (`TINA_WASM_HOST_ENGINE=<engine.wasm>
cargo build -p tina-wasm-host`, about 2 minutes incremental). The
repository's case suite runs its components under `tina-case-runner`,
which keeps the old positional-component surface:

```
tina-wasm-host                                       # default discovery
tina-wasm-host [--capabilities=doc.json | --preopen=g=h]... [--env=NAME=VALUE]...
tina-case-runner <component.wasm> [--preopen=g=h]... [--env=NAME=VALUE]... [--no-stdin]
```

**Default discovery** (no `--capabilities`, no `--preopen`): boot a
working session like native tina — the current directory is the
read-write workspace, the user's `~/.tina/config` is the settings tree,
preopened read-only with symlinks resolved. With no tina config the host
fails with a friendly message pointing at the path (`run tina once to
create one, or pass --preopen=config=<dir>`). The flags only fill in
what is missing.

`--preopen` exposes a host directory inside the guest as a WASI preopen
(read-write, yesterday's surface, still fully supported); `--env` sets a
WASI environment variable. `--capabilities` replaces the flag surface
with the engine's compiled capability document (see
`docs/capability_merger.md`): the host validates it fail-closed, derives
the preopen set from its directory grants (write grants rw, read grants
ro), enforces its command rules at `tina:process`, its network domain
allowlist at `tina:http`, its plugin grants at plugin admission, and
accepts `tina:policy.widen` escalations from the engine. Credentials
always arrive through `--env` from the caller's environment, never from
a file.
