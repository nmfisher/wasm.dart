# Stage 0 — foundation pins

Absolute references for everything the engine-on-wasm program builds on.
Verified on 2026-11-07 on this container (`/workspace`, branch
`asb/wasm-stage0-sdk-pin`).

## Pins table

| What | Pin | Notes |
|---|---|---|
| Dart SDK | `3.14.0-edge.e7112ac2438cd4f5ac0c832c27ce75bb10b02420` (channel: `main`/edge) | Source-built in this container at `/home/agent/dart-sdk-src/sdk`, out dir `out/ReleaseX64` |
| SDK commit | `e7112ac2438cd4f5ac0c832c27ce75bb10b02420` (2026-09-25, *"Report undocumented declaring parameters in `public_member_api_docs`"*) | The commit the lineage branch's suite was run against (`SDK_SUITE_REPORT.md`) |
| Embedder surface | 82 external functions in `sdk/lib/_internal/wasm/standalone/embedder.dart` | Byte-identical across `3.13.7` stable, the pinned edge build and `3.14.0-315.3.beta`; **differs** in `3.15.0-118.1.beta` (nullable annotations dropped from `WasmExternRef` params) |
| wasm.dart (this repo) | branch `asb/wasm-stage0-sdk-pin` @ `e7506b3` | Cut from lineage tip `rebase/dart-imports-sdk314` @ `e7506b3` |
| wasm.dart upstream | `simolus3/wasm.dart` main @ `29c9ce0` (*"Fix publishing autoation"*, 2026-10-25) | Merge-base of this lineage and upstream main is `de041ee` (*"A few more string functions"*); upstream main carries the same feature work as this fork's PR stack |
| wasmtime (Rust host) | `47.0.2` (crates.io, `Cargo.lock`) | `pkg/test_runner` — the component host the case suite runs on |
| wasmtime CLI | `47.0.4` (`/home/agent/bin/wasmtime`, 254cb7a02 2026-08-20) | Stock CLI; documented as unable to host `wasm:dart/trace@1.0.0` |
| wasm-tools CLI | cargo-installed `wasm-tools` at `~/.cargo/bin` | Used for WAT inspection only |
| runtime helpers | `pkg/runtime_helpers` (Rust, wasm32-unknown-unknown, nightly `-Zbuild-std`) | Prebuilt asset at `pkg/wasm_tools/assets/runtime_helpers.wasm` (gitignored, rebuilt by `tool/build_runtime_helpers.sh`) |

## Why this SDK

Choice rule: *the newest stable SDK whose embedder surface matches what the
lineage branch's tests exercise*.

- Newest **stable** at pin time is `3.13.7` (2026-10-22). It fails the
  workspace's `pub get`: `pkg/wasm_tools` requires `sdk: ^3.14.0-251.0.dev`
  ("pub get fails on anything older" — README note). Raw error:

  ```
  Resolving dependencies...
  The current Dart SDK version is 3.13.1.

  Because wasm_tools requires SDK version ^3.14.0-251.0.dev, version solving failed.
  ```

  So no stable SDK can even resolve this workspace; the pin cannot be stable.
- The lineage name `rebase/dart-imports-sdk314` is accurate: the suite the
  lineage ran (`SDK_SUITE_REPORT.md`) used exactly
  `3.14.0-edge.e7112ac2438cd4f5ac0c832c27ce75bb10b02420 (main)`, source-built.
  The workspace constraint `^3.14.0-251.0.dev` exists because
  `3.14.0-251.0.dev` (commit `de942dbb`) is where dart2wasm switched to
  `try_table`/`throw_ref`, which is what makes components runnable under
  Wasmtime (README note). Anything ≥ that and < 3.15 satisfies the constraint.
- `3.14.0-315.3.beta` also has a byte-identical embedder surface (82 externals)
  and would satisfy the constraint; it is a valid drop-in alternative recorded
  here, but the pin is the edge build the suite actually exercised.
- `3.15.0-118.1.beta` changes the embedder surface (see diff excerpt below) and
  is out.

  ```
  54c54
  < external WasmVoid clearSchedule(WasmExternRef? schedule);
  ---
  > external WasmVoid clearSchedule(WasmExternRef schedule);
  78c78
  < external WasmI32 stringLength(WasmExternRef? string);
  ---
  > external WasmI32 stringLength(WasmExternRef string);
  ```

## SDK source build

Location: `/home/agent/dart-sdk-src` (gclient checkout, `.gclient` solution
`https://dart.googlesource.com/sdk.git`, `managed: False`).

Already built at the pinned ref (clean tree, `HEAD detached at e7112ac2`) with:

```
python3 tools/build.py -m release -a x64 --no-rbe create_sdk
```

`args.gn` (ReleaseX64): `is_release = true`, `is_debug = false`,
`is_product = false`, `dart_use_compressed_pointers = false`,
`dart_use_crashpad = false`, `use_rbe = false`, `target_os = "linux"`,
`host_cpu = "x64"`, `target_cpu = "x64"`, sysroot `debian`.

Products verified present at pin time: `out/ReleaseX64/dart-sdk` (full SDK
layout incl. `bin/dart`, `bin/dartaotruntime`,
`bin/snapshots/dart2wasm_product.snapshot`, `lib/libraries.json`),
`out/ReleaseX64/dart` (VM), `dart2wasm.snapshot` / `dart2wasm_product.snapshot`
platform dills (`dart2wasm_standalone_platform.dill` etc.).

Stage-0 rebuild attempt of the `runtime` + `dart2wasm` targets at the pinned
ref: **now succeeds** (see "SDK rebuild log" at the end for the full story).
The initial attempt failed — exact blocker, recorded verbatim below — because
the `runtime` target's dependency graph pulls the
`//utils/dart2wasm:ffi_native_test_wasm_module` step, which compiles a C test
module with emscripten. This checkout was synced with the DEPS default
`download_emscripten = False`, so the `install_emscripten`/`activate_emscripten`
gclient hooks never ran and no `emcc` existed in the container.

Raw error of the initial attempt (both `runtime` and `dart2wasm` targets,
~1630/1643 steps in):

```
FAILED: [code=1] wasm/ffi_native_test_module.wasm
python3 ../../build/gn_run_binary.py ../../third_party/emsdk/upstream/emscripten/emcc --no-entry ../../tests/web/wasm/ffi/ffi_native_test_module.c -o wasm/ffi_native_test_module.wasm -O
Binary not found: /home/agent/dart-sdk-src/sdk/third_party/emsdk/upstream/emscripten/emcc
ninja: build stopped: subcommand failed.
buildtools/ninja/ninja -C out/ReleaseX64 runtime dart2wasm
BUILD FAILED
```

Resolution (follow-up task, 2026-11-07): two missing system packages —
`bzip2` and `lbzip2` — were why the checkout's own DEPS emsdk hook
(`python3 third_party/emsdk/emsdk.py install 3.1.3` at the DEPS-pinned rev
`e41b8c6`, version 3.1.3 per DEPS `emsdk_ver`) could not unpack its
`wasm-binaries.tbz2`: this GNU tar shells out to `lbzip2` for `.tbz2`, and
neither helper existed in the container (`.tar.xz` unpacking used xz and
worked, which masked it). After `sudo apt-get update && sudo apt-get install
-y bzip2 lbzip2` (Ubuntu noble, standard system packages), the in-tree
`emsdk.py install/activate 3.1.3` completed and put `emcc` exactly where the
build expects it (`third_party/emsdk/upstream/emscripten/emcc`), with no
changes to the build, DEPS, or source tree. A separate emsdk 7.0.0
(`./emsdk install latest && ./emsdk activate latest`) was also installed at
`~/tools/emsdk` per the follow-up brief's step 1, but the build itself uses
the DEPS-pinned in-tree 3.1.3.

## wasm.dart toolchain pins

- This repo: `nmfisher/wasm.dart`, branch `asb/wasm-stage0-sdk-pin`, tip of the
  `rebase/dart-imports-sdk314` lineage (PR stack #3,#4,#6,#7,#9,#10,#11,#12,#13,
  #14 rebased on upstream main).
- Upstream `simolus3/wasm.dart` main @ `29c9ce0` (2026-10-25). Note: the
  merge-base between this lineage and upstream main is `de041ee`; upstream has
  since merged the same feature work (23 commits between `de041ee` and
  `29c9ce0`) that this fork developed in its PR stack.
- `pkg/test_runner`: Rust reference host, wasmtime `47.0.2` +
  `wasmtime-wasi` `47.0.2` per `Cargo.lock`.
- wasmtime CLI `47.0.4` at `/home/agent/bin/wasmtime` — used for raw-module
  inspection; **cannot host components built by this pipeline** (missing
  `wasm:dart/trace@1.0.0` host import; see README note and the exit test
  below).

## Baseline suite

Per the brief: `component_test` 17/17 cases pass; full `dart test` = 52 pass /
1 fail, where `cli_print_test` is EXEMPT (stock wasmtime cannot host
`wasm:dart/trace@1.0.0`). Command line with `--timeout 5m`.

**Note on numbers at this branch tip** (deviation from the brief's stated
baseline, recorded per the brief's rules): the lineage tip carries **26** case
files (`pkg/wasm_components/test/cases/*.dart`), not 17, and
`cli_print_test.dart` is **not present** in the rebased lineage
(`rebase/dart-imports-sdk314` @ `e7506b3`). It exists in the fork's PR-stack
branches (`fork/pr/05-print-component-tasks` … `fork/pr/14-tz-pairing-datetime`,
introduced by `e4b929d`), along with `pkg/wasm_tools/example/http_service/test/
app_test.dart` — the rebase onto upstream main evidently dropped both. The
17/17 and 52/1 numbers therefore describe a suite state that does not exist
verbatim at this tip; what this stage verifies is that **every test present at
this tip still passes** under the pinned SDK — the no-regression invariant —
with the exact numbers recorded below.

## Exit test (hello world through the pinned pipeline)

Commands and raw output captured 2026-11-07:

```bash
export PATH=/home/agent/dart-sdk-src/sdk/out/ReleaseX64/dart-sdk/bin:/home/agent/.cargo/bin:/home/agent/bin:$PATH
cd /workspace/pkg/wasm_tools/example/hello_world_wasi

# 1. Raw dart2wasm with the pinned SDK (the --standalone step of the pipeline)
dart compile wasm --standalone -o /tmp/exit_test/hello.dart2.wasm /tmp/exit_test/hello.dart
# stdout: Generated wasm module 'hello.dart2.wasm'. See .../standalone.md for usage instructions.
#         The standalone option is experimental, and used imports may changeacross Dart SDK releases.
# (10 `dart.*` imports survive DCE in the hello module: print,
#  stringFromAsciiBytes, i64ToString, stringBufferCreate, stringLength, ...)

# 2. Full wasm.dart pipeline (embedder self-linking into a component)
dart run wasm_tools compile bin/print_probe.dart \
  --output /tmp/exit_test/probe.wasm \
  --hooks-include-dev-dependencies --no-implicit-wasi-imports
# stdout tail:
#   {"assets_for_linking": {}, "status": "success", "timestamp": "2026-11-07 08:24:30.000"}
#   [FINE] compiler: Patching 26 functions
#   [FINE] compiler: Patched 547 references
#   [INFO] compiler: Writing component to /tmp/exit_test/probe.wasm

# 3. Run under wasmtime via the pipeline's reference host (test_runner)
cd /workspace && ./target/debug/test_runner /tmp/exit_test/probe.wasm
# stderr: Error: Missing tests export   (rc=1)
#         — print_probe is a plain app, not a wasmdart:tests world component;

# 3'. Run under stock wasmtime CLI (documented exemption)
wasmtime /tmp/exit_test/probe.wasm
# stderr:
#   Error: failed to run main module `probe.wasm`
#   Caused by:
#       0: component imports instance `wasm:dart/trace@1.0.0`, but a matching implementation was not found in the linker
#       1: instance export `capture-utf16` has the wrong type
#       2: function implementation is missing
```

The `wasm:dart/trace@1.0.0` failure under stock wasmtime is the documented
exemption class (README note at top; `cli_print_test`'s exemption): the embedder
keeps `dart.stackTrace*` reachable from core error paths, so every component
carries the trace dependency, and only a host linking `wasm:dart/*` — the
case suite's Rust runner — can run it.

### Exit test result: PASS

The run path that exercises "hello world through the wasm.dart pipeline under
wasmtime" uses the suite's `print` case — a literal hello world (two `print`
lines lowered to `wasi:cli/stdout` writes) — compiled with the **pinned SDK**
through the full pipeline (embedder self-linked by `wasm_tools`), executed by
the wasmtime 47.0.2 reference host:

```bash
export PATH=/home/agent/dart-sdk-src/sdk/out/ReleaseX64/dart-sdk/bin:/home/agent/.cargo/bin:/home/agent/bin:$PATH
cd /workspace/pkg/wasm_components
dart run wasm_tools compile test/cases/print.dart \
  --output /tmp/exit_test/print.wasm \
  --hooks-include-dev-dependencies --no-implicit-wasi-imports
# [FINE] compiler: Patching 26 functions
# [FINE] compiler: Patched 630 references
# [INFO] compiler: Writing component to /tmp/exit_test/print.wasm   (rc=0)

cd /workspace && ./target/debug/test_runner /tmp/exit_test/print.wasm
# {"type":"start","test":0}
# {"type":"bool","value":true}
# hello from wasm
# second line
# {"type":"end","test":0}                                        (rc=0)
```

## Suite results under the pinned SDK

Run date 2026-11-07, environment:

```
export PATH=/home/agent/dart-sdk-src/sdk/out/ReleaseX64/dart-sdk/bin:/home/agent/.cargo/bin:/home/agent/bin:$PATH
```

1. `pkg/wasm_components` (the case suite; `setUpAll` builds `cargo build -p
   test_runner`, wasmtime 47.0.2):

   ```
   cd /workspace/pkg/wasm_components && dart test --timeout 5m
   → 11:42 +26: All tests passed!   (rc 0)
   ```

   **26/26 cases pass** at this branch tip (the brief's "17/17" refers to the
   lineage's case count at an earlier state; the tip carries 26 — see the
   baseline note above). No regression: everything passes.

2. `pkg/wasm_tools` (abi reader, component writer, integration component
   validation over the two freshly compiled `integration_tests/build/*.wasm`):

   ```
   cd /workspace/pkg/wasm_tools && dart test --timeout 5m
   → 00:00 +11: All tests passed!   (rc 0)
   ```

3. `integration_tests` components recompiled with the pinned SDK first:

   ```
   cd /workspace/integration_tests && dart run tool/compile.dart
   → enum_export.wasm, write_stream_immediate.wasm   (rc 0)
   ```

Total: **37 pass / 0 fail** across every test-bearing package at this tip.
The brief's baseline "52 pass / 1 exempt fail" includes tests that do not
exist at this tip (`cli_print_test`, and the case-count difference: 26 vs 17);
the exemption reasoning (stock wasmtime cannot host `wasm:dart/trace@1.0.0`)
was re-verified live during the exit test above, where stock `wasmtime run`
rejects a pipeline-built component with exactly that error.

## SDK rebuild log

**Status: the `runtime` + `dart2wasm` targets now build cleanly at the pinned
ref — `ninja: no work to do` on re-run.**

Attempted at the pinned ref in this container, from
`/home/agent/dart-sdk-src/sdk`:

```
export PATH=/home/agent/depot_tools:/home/agent/.cargo/bin:/home/agent/bin:$PATH
export DEPOT_TOOLS_UPDATE=0 GCLIENT_SUPPRESS_GIT_VERSION_WARNING=1
python3 tools/build.py -m release -a x64 --no-rbe runtime dart2wasm
```

Timeline:

1. **First attempt (Stage 0):** BUILD FAILED after ~1630/1643 steps on the
   emcc step (raw error above). All C++/Dart compilation in the graph had
   already succeeded; only the emscripten fixture step and the 14 links it
   gated were missing.
2. **Follow-up (this task):** installed emsdk — first `latest` (7.0.0) at
   `~/tools/emsdk` per the brief's step 1 (`clone`, `install`, `activate`,
   `source emsdk_env.sh`; emcc 7.0.0 verified), then the DEPS-pinned 3.1.3
   in-tree via the checkout's own hook commands, which is what the build's
   hard-coded path consumes. The in-tree install initially failed:

   ```
   tar (child): lbzip2: Cannot exec: No such file or directory
   tar (child): Error is not recoverable: exiting now
   ['tar', '-xf', '.../zips/2ddc66235392b37e5b33477fd86cbe01a14b8aa2-wasm-binaries.tbz2', '--strip', '1'] failed with error code 2!
   error: installation failed!
   ```

   Fixed by installing the missing system packages (`sudo apt-get update`,
   `sudo apt-get install -y bzip2 lbzip2`; no sudo-free alternative existed —
   apt without sudo failed on the dpkg lock, and both packages had
   "no installation candidate" until `apt-get update`). No source, DEPS, or
   build-config change was involved.
3. **Retry:** the same build command completed in **5.5 s** — everything up to
   the emcc step was already built by the first attempt; the emcc step
   produced `out/ReleaseX64/wasm/ffi_native_test_module.wasm` (7016 bytes,
   verified to also build standalone through `gn_run_binary.py`), and the 14
   gated link steps finished. A second invocation reported
   `ninja: no work to do` (1.4 s), confirming the graph is fully satisfied.

Build config at the pinned ref (unchanged by this task; `out/ReleaseX64/
args.gn`): `is_release = true`, `is_debug = false`, `is_product = false`,
`dart_use_compressed_pointers = false`, `dart_use_crashpad = false`,
`use_rbe = false`, `target_os = "linux"`, `host_cpu = "x64"`,
`target_cpu = "x64"`, sysroot `debian`. SDK git state: `HEAD detached at
e7112ac2` (the pinned commit), clean tree.

Smoke test of the rebuilt toolchain (2026-11-07): the built SDK's
`bin/dart` (version string
`3.14.0-edge.e7112ac2438cd4f5ac0c832c27ce75bb10b02420 (main)`) compiled a
hello-world with `dart compile wasm --standalone` (`hello.dart2.wasm`,
16786 bytes), `wasm_tools compile` self-linked the print case into a
component (`print.wasm`, 487481 bytes), and the wasmtime 47.0.2 reference
host ran it:

```
{"type":"start","test":0}
{"type":"bool","value":true}
hello from wasm
second line
{"type":"end","test":0}                                        (rc=0)
```

The prior, successful build of the SDK products at this ref used:

```
python3 tools/build.py -m release -a x64 --no-rbe create_sdk
```

(September 26, 2026; produces `out/ReleaseX64/dart-sdk/`, which Stage 0's exit
test and suite runs use.)
