# Stage 6 — dogfood runner notes (wasm.dart, `asb/wasm-stage6-dogfood`)

No host-import changes this stage: the engine component grew (the
tina-side front end, approvals and persistence slices) against the
existing `tina:http`, `tina:process` and WASI surfaces. What this
repository contributes to stage 6 is the performance answer.

## The spawned-runner startup cost was the runner's build profile

Stage 5 recorded 75.4 s to `ready` from a spawned runner and blamed
"the debug-build runner's cranelift JIT". Confirmed and fixed:

| runner | boot to full exit (shutdown served) | peak RSS |
|---|---|---|
| debug (`cargo build`) | 81-84 s | ~393 MiB |
| release (`cargo build --release -p tina-wasm-host`) | **4.3-5.1 s** | ~352 MiB |

Same component (1.82 MB engine), same host, direct or spawned — the
16× difference is the debug binary's unoptimized cranelift compiling
the component at instantiation. `tool/measure_startup.sh` (tina repo)
is the measurement.

The stage-5 "~45 s sandbox grandchild delay" does not reproduce with
the release runner; the spawned-vs-direct gap it attributed to the
sandbox collapses into the JIT cost above.

## The RSS floor is the embedder runtime

The 1.1 MB stage-3 probe component peaks at ~251 MiB under the same
release runner; the engine adds ~100 MiB of code and heap on top. A
runner-side reduction is not available: the dominant allocation is the
dart2wasm embedder runtime and instance metadata. Recorded as the
floor; component-size work (tree shaking, smaller closures) would move
it only marginally.

## Recipe update

- Build the runner release when serving the engine:
  `cargo build --release -p tina-wasm-host`
- The engine component itself still compiles with the pinned
  3.14-edge SDK (`wasm_tools` requires it; `dart2wasm` from older
  SDKs emits the legacy `try` instruction wasmtime rejects —
  see README).
- The tina-side stage-6 driver, scenarios and records live in the
  tina repository (`packages/libraries/tina_wasm/tool/`,
  `docs/wasm_stage6_dogfood.md`).
