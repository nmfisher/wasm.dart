# Plugin boundary runtime safety and the compile-on-load dev loop

Status: implemented on `codex/issue85-loader-current-main` (issue #85).
This document describes the execution budget model of the per-plugin-Store
boundary (`pkg/tina_wasm_host/src/plugin_loader.rs`, `tina_plugin.rs`) and the
opt-in watched-artifact dev loop.

## Execution budgets

Every plugin runs in its own Wasmtime `Store` inside a `PluginBoundary`.
The boundary engine must be built with `plugin_engine_config()`, which
enables epoch interruption together with the GC/function-references/
exceptions feature set the Dart toolchain needs.

| Budget | Default | Mechanism | Notes |
|---|---|---|---|
| Per-call wall clock | 10 s | `Store::set_epoch_deadline` before each `describe`/`run` | Exceeding it traps the call; the Store is retired and the lease revoked. |
| Instantiation wall clock | 60 s | deadline armed at Store creation | Instantiation runs the Dart runtime start; it is host-initiated on an admitted artifact, so its budget is separate from the call budget. |
| Linear memory | 256 MiB | `StoreLimits` (installed before instantiation) | Bounds guest shadow stacks and linear memories. Dart GC-heap objects live in the Wasm GC heap, which is bounded by the wall-clock budget, not by a linear-memory cap. |
| Tables | 1,000,000 entries | `StoreLimits` | |
| Instances / memories / tables (counts) | 8 / 8 / 16 | `StoreLimits` | dart2wasm components instantiate multiple core instances; the counts are generous ceilings, not targets. |

Wasmtime 47.0.2 has no separate GC-heap budget knob; unbounded allocation is
bounded by the epoch deadline (a loop that only allocates still burns wall
clock and is killed like any other runaway). Fuel accounting is not enabled:
the issue selected epoch interruption as the cancellation mechanism.

A ticker thread per boundary advances the engine epoch every 10 ms
(`PluginLimits::epoch_tick`); kill latency is therefore one tick plus guest
interrupt latency, measured by the `epoch_kill` event in `perf_probe`.

## Cleanup and failure semantics

- A trapped or epoch-killed call retires its Store inside `PluginBoundary`
  and revokes the host lease in `TinaPluginRuntime`; later calls on that
  lease are refused before touching the Store.
- `close` never waits on plugin code. If a guest call is active, the lease
  is revoked immediately and the Store retirement is deferred to the end of
  that call (`pending_retires`, drained under the boundary lock). The call
  result is discarded.
- `open` never waits on plugin code: while the boundary is busy it returns
  `Busy` (surfaced as `unavailable` on the host interface) instead of
  blocking the engine on a guest.
- Store ownership never changes: Wasmtime handles stay inside the owning
  Store, and nothing crosses between the engine Store and plugin Stores.

## Compile-on-load dev loop

By default the runner freezes the plugin artifact bytes at startup
(production prebuilt behavior). With `--plugin-watch`, every
`session.open()` re-reads the `--plugin-component` file, so the loop is:

    edit plugin source
      -> wasm_tools witgen + compile + stamp-abi   (no engine rebuild)
      -> next hook attach loads the new component into a fresh Store

The engine keeps running; open plugin instances keep their Stores and state
until they are closed or trap, which is what makes replacement safe. A
broken rebuild fails `open` (surfaced as `failed`) without disturbing
already-attached instances.

`testdata/plugin_probe/regenerate_fixture.sh` is the reference authoring
path: WIT -> `wasm_tools witgen` -> whole-program `wasm_tools compile` ->
`wasm_tools stamp-abi` (the Dart stamp is byte-identical to the original
`stamp_abi.py`).

## Measurement entry point

`cargo run --bin perf_probe -- <plugin.component.wasm> <spin.component.wasm>
[calls] [cycles]` prints load time, per-call overhead (mean/median/p95),
repeated unload/reload reclamation with resident-memory deltas, and
epoch-kill latency as JSON lines. It asserts no thresholds; the issue sets
none.
