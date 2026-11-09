# Stage 5 — the real engine as a component

Program context: tina's headless engine (the same `SessionAssembly` +
`--pipes` protocol the CLI serves) now builds as a wasm component and
runs a full agent turn under wasmtime: it reads its config from a
preopened config tree, calls a provider through `tina:http`, runs a
tool call (the real `write` tool from `tina_tools`) over the WASI
filesystem preopen, and completes the turn — driven by the same pipes
client that drives the native engine.

## What landed (wasm.dart, `asb/wasm-stage5-component`)

- `pkg/tina_wasm_host/src/main.rs`: optional stdin wiring. `wasi.stdin` is
  connected unless `--no-stdin` is passed: a plain thread pumps process
  stdin into a channel (`ChannelReader`) that the guest reads —
  `tokio::io::stdin` could not be used (its reads are bound to the
  single-threaded runtime's event loop; wiring it stalls every guest,
  even ones that never read stdin).
- `pkg/wasm_components/lib/src/runtime/async/stream.dart` (guest
  runtime, second fix after stage 3's waitable-set join):
  - `StreamReadState._completeClose` is now idempotent — the dropped
    event and the task's teardown can both route there, and the second
    pass freed the pending read's buffer twice (a talc
    double-deallocate crashed the guest);
  - the pending read's buffer is freed with an EMPTY element region:
    reads copy elements out eagerly (`AllocatedString.read` frees each
    element's own allocations as it copies), and a blocked read's
    unwritten region holds no elements at all — freeing it as if it
    held strings crashed the allocator.
- `pkg/wasm_components/lib/src/runtime/async/pipes note`: none further.

## Measurements (this box, debug-build runner and guest, `-O0` guest)

| metric | value |
|---|---|
| component size | 1,822,499 bytes (`bin/engine.wasm`) |
| build (dart2wasm + lift, warm pub/hook cache) | 7.3 s (~21 s cold) |
| startup, spawned runner → first `ready` | 75.4 s |
| startup, direct shell run → first `ready` | ~2-8 s |
| peak RSS (runner process, boot+turn) | 363 MiB |

The spawned-runner startup gap is environmental: this sandbox imposes a
~45 s delay on grandchild processes before the runner's first output
(the same binary, directly spawned, reaches `ready` in seconds; the
delay reproduces for unrelated components and precedes instantiation).
The real JIT cost is the debug-build cranelift compiling the ~2k-function
dart2wasm module (15-19 s per the case-suite notes). A release-build
runner and an optimized guest are the obvious follow-ups; neither is
this stage's question.

## The exit test (tina repo, `tool/engine_exit_test.sh`)

Both engines complete the identical scripted turn against the same fake
provider: `ready` → submit → provider call (`tina:http` for wasm,
`dart:io` for native) → `tool_use` (`write`) → the file lands in the
workspace (`stage5.txt`, "stage5 edit landed") → final answer →
shutdown. Driver: `tool/pipes_drive.dart`; provider:
`tool/fake_anthropic_turn_server.py` (a two-step scripted SSE
conversation). Result: `RESULT: PASS` on both sides.

## What is NOT in the component yet (the honest slice list)

The wasm registry registers the plugins whose dependency trees compile
without `dart:io`/`dart:ffi`: system-instruction, providers (policy +
limits), mode (the real `ModePlugin` policy), tools (the portable
`read`/`write`/`edit` core over the preopened filesystem), the
approvals-stream channel, session-controls. Excluded, in dependency
order of size:

- process tools (bash/exec), glob/ls/stat — `tina_tools`' sandboxed
  process runners and directory walks are `dart:io`-bound; the
  `tina:process` host import (stage 4) is the intended target;
- persistence — sqlite is FFI;
- classification's model classifiers, shell, MCP, self-update, and the
  TUI plugins.

The wasm side of tina carries the composition root
(`packages/libraries/tina_wasm`): `bin/engine.dart` (preload → assembly
→ pipes over WASI stdio), `PreloadedAppIo` (a synchronous snapshot of
the config tree, because the assembly boot is a chain of synchronous
constructors), `WasiToolFileSystem`, `PortableToolsPlugin`, and the
wasm plugin registry.
