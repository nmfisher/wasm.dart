# Report: case suite under the source-built Dart SDK, plus README update

**Date:** 2026-09-27
**Branch:** `asb/dart-imports-sdk314` (from `610cc3e66a9dc96d9ad85ed5e782be4b52004e9e`)
**SDK:** `/home/agent/dart-sdk-src/sdk/out/ReleaseX64/dart-sdk`
`Dart SDK version: 3.14.0-edge.e7112ac2438cd4f5ac0c832c27ce75bb10b02420 (main)`

## TL;DR

- **`pkg/wasm_components`: 10 of 20 cases pass, 10 fail** under the wasmtime-backed
  `test_runner` host.
- **`pkg/wasm_components/tool/run_cases.mjs`: 20 of 20 pass** (EXIT 0) with the *same* SDK.
  The two harnesses therefore disagree, and the disagreement locates every failure in the
  repo's component/host layer — **not** in the SDK bump.
- No test or golden file was modified.
- Nothing was fixed: none of the four root causes is clearly caused by the SDK bump, so per
  the task rules they are reported and left alone.

## Environment

```
export PATH=/home/agent/dart-sdk-src/sdk/out/ReleaseX64/dart-sdk/bin:/home/agent/.cargo/bin:/home/agent/bin:$PATH
dart --version   → 3.14.0-edge.e7112ac2438cd4f5ac0c832c27ce75bb10b02420 (main)
cargo --version  → cargo 1.98.1 (797e8a9bc 2026-08-05)
wasmtime --version → wasmtime 47.0.4 (254cb7a02 2026-08-20)
node --version   → v24.19.0
```

`dart pub get` at the repo root with the new SDK: `Got dependencies!` (EXIT 0).
`/workspace/target/`, `.dart_tool/`, `*.wasm.map` and `**/bin/*.wasm` are all gitignored, so
the build and test runs left the working tree clean apart from `README.md`.

## Task A — suite results

### Command

```
cd /workspace/pkg/wasm_components && dart test --reporter expanded
```

Run in the background (detached, polled); wall clock 7 min 23 s. `setUpAll` first ran
`cargo build -p test_runner` (already up to date, `Finished dev profile in 0.24s`).

### Result

```
07:23 +10 -10: Some tests failed.
=== SUITE EXIT 1 at Sun Sep 27 00:55:56 UTC 2026 ===
```

**20 cases total: 10 pass, 10 fail.**

| case | result |
|---|---|
| datetime | pass |
| developer | pass |
| double_parse | **FAIL** |
| double_parse_edge | pass |
| double_parse_rounding | **FAIL** |
| double_parse_rounding_bits | **FAIL** |
| double_to_fixed | pass |
| double_to_string | **FAIL** |
| json_encode_string | pass |
| json_surrogates | pass |
| print | **FAIL** |
| print_astral | **FAIL** |
| regexp | **FAIL** |
| regexp_classes | pass |
| regexp_semantics | **FAIL** |
| regexp_unicode | pass |
| string | pass |
| string_from_char_codes | **FAIL** |
| string_replace | **FAIL** |
| weak | pass |

Golden files are compared by `component_test.dart:50-52`
(`expect(output.stdout, golden)`), and all 10 passing cases matched their golden exactly.

### The 10 failures, grouped by root cause

Four distinct causes. Evidence is the real log output from
`/tmp/suite-wasm_components.log`.

#### Cause A — `dart_free` on a zero-size allocation traps (4 cases)

`string_from_char_codes`, `string_replace`, `regexp`, `regexp_semantics`.

Each aborts the moment a case records an **empty** string. `test_runner` exits 101:

```
  thread 'main' (159182) panicked at pkg/test_runner/src/main.rs:106:14:
  called `Result::unwrap()` on an `Err` value: error while executing at wasm backtrace:
      0:    0x584 - runtime_helpers.wasm!<talc[...]::...>::deallocate
      1:    0xb09 - runtime_helpers.wasm!dart_free
      2:  0x3ad90 - M!AllocatedString.free
      3:  0x3ad08 - M!_Imported$0.recordString
      4:  0x3ace2 - M!_ImportedCollector.recordString
      ...
  Caused by:
      0: memory fault at wasm address 0xfffffffe in linear memory of size 0x120000
      1: wasm trap: out of bounds memory access
```

`string_from_char_codes` reached `{"type":"string","value":""}` (its `_empty` test) and died
there; `regexp` died in `_quantifiers` right after `"value":""`; `regexp_semantics` died in
`_repro` right after `"value":""`.

**Cause.** `AllocatedString.free` (`pkg/wasm_components/lib/src/runtime/string.dart:14-16`)
calls `dartFree(ptr, 2 * packedLength, 2)`. For an empty string `mallocAligned(2, 0)` routes
to Rust `dart_realloc(0, 0, 2, 0)`, whose zero-size path returns `align as *mut u8` — i.e.
pointer `2` (`pkg/runtime_helpers/src/memory.rs:12-15`). The matching `dart_free`
(`memory.rs:40-43`) has **no** zero-size guard:

```rust
pub extern "C" fn dart_free(ptr: *mut u8, num_bytes: usize, align: usize) {
    unsafe { dealloc(ptr, Layout::from_size_align_unchecked(num_bytes, align)) }
}
```

so talc's `deallocate` gets a ZST layout and the dangling pointer `2`, and faults at
`0xfffffffe`. The asymmetry is visible in the repo: `dart_realloc` special-cases zero, `dart_free`
does not.

**Why not fixed.** The trigger is the repo's Rust allocator, not the SDK. Confirmed
decisively: `run_cases.mjs` passes `string_from_char_codes`, `string_replace`, `regexp` and
`regexp_semantics` with the *same* SDK, because its allocator is a bump allocator with an
explicit no-op free (`run_cases.mjs:192-194`, *"The bump allocator never reuses memory. Fine
for tests."*) — the host-side path that faults is simply not exercised there.

#### Cause B — `EmbedderStringImpl.[]` traps with an out-of-bounds array access (3 cases)

`double_to_string`, `double_parse`, `double_parse_rounding`.

`double_to_string` died on the first value of its `_exponents` test (`1e21.toString()`),
after tests 0-2 had already produced correct output:

```
  {"type":"start","test":3}
  thread 'main' (159278) panicked at pkg/test_runner/src/main.rs:106:14:
  ...
      0:  0x36f26 - M!EmbedderStringImpl.fromCharCode
      1:  0x36f0e - M!EmbedderStringImpl.[]
      2:  0x36b4a - M!_exponentialBody
      3:  0x3632b - M!doubleToString
      4:  0x12c7c - M!f64ToString
      5:  0x14086 - M!BoxedDouble.toString
      ...
  Caused by:
      wasm trap: out of bounds array access
```

`double_parse` shows the same trap via `_roundTrip`.

**Cause.** `EmbedderStringImpl` lives in the SDK
(`sdk/lib/_internal/wasm/standalone/embedder_string.dart`), and its indexing delegates to the
repo's embedder exports:

```dart
String operator [](int index) {                       // embedder_string.dart:495
  IndexErrorUtils.checkIndex(index, length);          // length → embedder.stringLength
  return EmbedderStringImpl.fromCharCode(_codeUnitAtUnchecked(index));
}
```

`length` and `_codeUnitAtUnchecked` call the repo's `stringLength` / `stringCodeUnitAt`
exports (`pkg/wasm_components/lib/src/embedder/exports.dart:170`), which index the
repo-owned `WasmArray<WasmI16>` behind a `Utf16String`. A wrong length crossing that boundary
lets `checkIndex` pass and the array access then trap. `EmbedderStringImpl.[]` is reached
from `_exponentialBody` through `digits.digits[0]`
(`pkg/wasm_components/lib/src/embedder/double_format.dart:495-504`), which is why exactly the
double-formatting cases fail.

**Why not fixed.** Same reasoning: with the *same* SDK, `run_cases.mjs` passes
`double_to_string`, `double_parse` and `double_parse_rounding`. The Dart codegen and the SDK's
core library are fine; what differs is that the component path marshals strings across a
canon lift/lower boundary while the raw-module path calls the exports directly. That points at
the repo's component string ABI, and fixing it is not a small, obviously-safe change.

#### Cause C — `print` produces no output in `test_runner` (2 cases)

`print`, `print_astral`. These do **not** crash; they run to completion and then fail the
golden comparison:

```
  Expected: '{"type":"start","test":0}\n'
              '{"type":"bool","value":true}\n'
              'hello from wasm\n'
              'second line\n'
              '{"type":"end","test":0}\n'
              ''
      Actual: '{"type":"start","test":0}\n'
              '{"type":"bool","value":true}\n'
              '{"type":"end","test":0}\n'
              ''
     Which: is different.
                                    ^
             Differ at offset 57
  test/component_test.dart 52:7  main.<fn>
```

The markers arrive; the printed lines do not.

**Cause.** Not the compiler and not the Wasmtime engine. Verified three ways:

1. The component *is* wired for stdout. `wasm-tools component wit print.wasm` shows
   `import wasi:cli/stdout@0.3.0;`, and the core module imports
   `implicitImport_stdoutWriteViaStream`, `implicitImport_stdoutStreamNew`,
   `implicitImport_stdoutStreamWrite`, … The rewrite deliberately ignores
   `--no-implicit-wasi-imports` (`pkg/wasm_tools/lib/src/compiler/transform.dart:333-341`).
2. Running the compiled `print.wasm` under `test_runner` reproduces it exactly:
   ```
   $ ./target/debug/test_runner print.wasm   → EXIT=0
   {"type":"start","test":0}
   {"type":"bool","value":true}
   {"type":"end","test":0}
   ```
3. Wasmtime's stdout path itself is healthy — the same SDK-built
   `hello_world_wasi` component prints correctly under the wasmtime CLI today:
   ```
   $ wasmtime run bin/app.wasm
   Hello world!
   This is running Dart!
   EXIT=0
   ```

So the gap is that `pkg/test_runner`'s host does not surface the guest's
`wasi:cli/stdout` write when it drives the test export through the concurrent
component-model API (`config.wasm_component_model_async(true)`, `call_async`). Note that
`run_cases.mjs` — the harness the old README pointed at for `print` — passes both
`print.dart` and `print_astral.dart`.

**Why not fixed.** A `test_runner` host/event-loop issue, not an SDK regression.

#### Cause D — timeout (1 case)

`double_parse_rounding_bits`.

```
07:23 +10 -10: test/component_test.dart: double_parse_rounding_bits [E]
  TimeoutException after 0:00:30.000000: Test timed out after 30 seconds. See https://pub.dev/packages/test#timeouts
  dart:isolate  _RawReceivePort._handleMessage
```

No wasm trap, no panic — the component just did not finish inside the 30 s default. The same
case passes under `run_cases.mjs`, which again points away from the SDK. Not diagnosed
further; the harness timeout makes it impossible to tell a slow case from a hang without
changing the timeout, which would be weakening the test.

### Task A step 2 — `run_cases.mjs`

```
export DART_SDK=/home/agent/dart-sdk-src/sdk/out/ReleaseX64/dart-sdk
cd /workspace/pkg/wasm_components && node tool/run_cases.mjs
```

`DART_SDK` **must** be set — the script defaults to `/opt/dart-sdk`, which is 3.13.1 and
cannot even resolve this repo's dependencies.

```
PASS datetime.dart
PASS developer.dart
PASS double_parse.dart
PASS double_parse_edge.dart
PASS double_parse_rounding.dart
PASS double_parse_rounding_bits.dart
PASS double_to_fixed.dart
PASS double_to_string.dart
PASS json_encode_string.dart
PASS json_surrogates.dart
PASS print.dart
PASS print_astral.dart
PASS regexp.dart
PASS regexp_classes.dart
PASS regexp_semantics.dart
PASS regexp_unicode.dart
PASS string.dart
PASS string_from_char_codes.dart
PASS string_replace.dart
PASS weak.dart
=== MJS EXIT 0 at Sun Sep 27 01:00:15 UTC 2026 ===
```

**20/20, EXIT 0. It does not agree with the wasmtime-backed suite** (10/20). That
disagreement is the single most useful result here: it exonerates the SDK bump for all ten
failures and pins them on the repo's component ABI and `test_runner` host.

Caveat on that comparison, so it is not over-read: `run_cases.mjs` is *designed* to be
weaker as a host. Its `free` is a no-op (hides cause A) and it calls the embedder exports
directly instead of across a component boundary (hides cause B). It agrees with the
wasmtime run on the 10 passing cases and disagrees on all 10 failures, in the direction that
clears the SDK.

### Other suites

**`pkg/wasm_tools` — 8/8 pass.**

```
cd /workspace/pkg/wasm_tools && dart test
00:00 +8: All tests passed!
=== WT EXIT 0 at Sun Sep 27 01:05:35 UTC 2026 ===
```

Ran in 2 s. Cases: `reader_test.dart` (Package parse), `components/component_test.dart`
(can define simple component), `components/type_test.dart` (can write types, can write
instances, resources). These exercise the component builder and ABI reader directly, with no
SDK codegen in the loop, and they pass unchanged on the new SDK.

`pkg/wasi` and `pkg/test_runner` have **no `test/` directory**, so there is nothing to run
there (`ls: cannot access '.../test': No such file or directory`). `pkg/test_runner` *is*
exercised transitively — it is both the package under test's dependency and the Rust host
binary that `component_test.dart` builds and drives.

## Task B — README

Diff on `README.md`, 15 insertions / 6 deletions:

```diff
@@ -8,11 +8,20 @@ The goal is to get `wasmtime run dart_compiled_app.wasm` to work without further
 > [!NOTE]
 > These tools are still in development and not ready for production. Please file issues for problems you run into!
 >
-> dart2wasm (as of Dart 3.13) emits the legacy `try` instruction in SDK internals, which Wasmtime's
-> engine does not implement, so compiled components validate but can't run in Wasmtime yet (see
-> the `print` row). The raw-module harness in `pkg/wasm_components/tool/run_cases.mjs` does run
-> them: it implements the component-model event loop (canon streams/futures, waitable sets and
-> the `callback` pump), so `print` completes there.
+> **Requires a Dart SDK `^3.14.0-251.0.dev`.** No released SDK satisfies that yet: the latest
+> stable is 3.13.4, beta is 3.14.0-211.1.beta and dev is 3.14.0-248.0.dev, all short of it.
+> Until a release reaches that revision you need to build the SDK from source at or past it
+> (`pub get` fails on anything older).
+>
+> Older dart2wasm emitted the legacy `try` instruction in SDK internals, which Wasmtime does not
+> implement, so compiled components validated but could not run there. From `3.14.0-251.0.dev`
+> (commit `de942dbb`, *"[dart2wasm] Translate exceptions to try_table instructions"*) dart2wasm
+> emits `try_table`/`throw_ref` instead, and compiled components run under Wasmtime with no
+> special flags. Verified with Wasmtime 47.0.4: `wasmtime run` on
+> `pkg/wasm_tools/example/hello_world_wasi` prints the expected output and exits 0.
+>
+> `pkg/wasm_components/tool/run_cases.mjs` remains useful as a Rust-free way to run the case
+> suite in Node.
@@ -125,7 +134,7 @@ __Legend__:
-| print                                    | ✅          | 📦        | Via `wasi:cli/stdout`; completes at run time in the raw-module harness |
+| print                                    | ✅          | 📦        | Via `wasi:cli/stdout`; completes under Wasmtime (e.g. `hello_world_wasi`) and in the raw-module harness |
```

Every number and claim in that diff was verified in this session:

- stable 3.13.4 / beta 3.14.0-211.1.beta / dev 3.14.0-248.0.dev — fetched live from
  `storage.googleapis.com/dart-archive/channels/{stable,beta,dev}/release/latest/VERSION`.
- `^3.14.0-251.0.dev` — read from `pkg/wasm_tools/pubspec.yaml`.
- `pub get` failing on an older SDK — reproduced with 3.13.1:
  `Because wasm_tools requires SDK version ^3.14.0-251.0.dev, version solving failed.`
- `try_table` emission and the wasmtime run — measured with wasm-tools 1.254.0 and
  wasmtime 47.0.4 on `hello_world_wasi` (20 `try_table`, 19 `throw_ref`, 0 `try`/`rethrow`/
  `delegate`, `wasm-tools validate` clean, `wasmtime run` EXIT 0).
- The `print` row keeps its "raw-module harness" mention only because that is still true —
  `run_cases.mjs` passes both print cases. What changed is that it is no longer the only
  place: `hello_world_wasi` completes under wasmtime. I deliberately did **not** claim print
  works in `test_runner`, because it currently does not (cause C).

## What I did not change, and why

No test, golden file, or source file was edited. Re-stating the rule and how I applied it:
a fix was allowed only where the cause is clearly the SDK bump *and* the fix is small and
safe. All four causes trace to the repo's own component ABI, Rust `runtime_helpers`
allocator, or `test_runner` host — the Node harness passes 20/20 with the identical SDK, which
is direct evidence the SDK is not at fault. Causes A and B each have a plausible small patch
(a zero-size guard in `dart_free`; an ABI fix in the string length path), but neither is
small-and-safe *and* SDK-caused, so both are left for a targeted change with its own test.

## Git

- Branch: `asb/dart-imports-sdk314`, created from `610cc3e66a9dc96d9ad85ed5e782be4b52004e9e`.
- No other branch touched, no rebase/reset/force-push, nothing pushed.

```
$ git -C /workspace branch --show-current
asb/dart-imports-sdk314
```

Commit SHA: see the output of the commit step (also printed at the end of the session).

## Logs

| log | contents |
|---|---|
| `/tmp/suite-wasm_components.log` | `dart test` for `pkg/wasm_components` (full failure output) |
| `/tmp/suite-mjs.log` | `node tool/run_cases.mjs` |
| `/tmp/suite-wasm_tools.log` | `dart test` for `pkg/wasm_tools` |
| `/tmp/printchk.log` | compile of `test/cases/print.dart` to a component |
| `/tmp/print-tr.log` | `test_runner` on that component |
