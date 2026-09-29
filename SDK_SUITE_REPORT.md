> **Historical report:** The investigation below predates removal of the Node
> test runner. Its Node commands and results are retained as historical evidence,
> not current test instructions. Use the Rust/Wasmtime integration command in
> README.md for current validation.

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
- **Follow-up:** causes A and B have since been fixed, taking the wasmtime-backed suite from
  10/20 to **15/20**. Two of the three remaining `double_parse*` failures turned out to be
  masking a further, unrelated cause (E, the `test_runner` host's f64 rendering). See
  [Follow-up: causes A and B fixed](#follow-up-causes-a-and-b-fixed--1520) at the end.

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

## Follow-up: causes A and B fixed — 15/20

Both causes were diagnosed and fixed after the findings above were written. Those findings
stand unchanged; the pass counts here supersede them. Causes C and D are untouched.

```
Branch: asb/dart-imports-sdk314
dart      3.14.0-edge.e7112ac2438cd4f5ac0c832c27ce75bb10b02420 (main)
wasmtime  47.0.4
```

### Fix 1 — cause A: `dart_free` on a zero-size allocation

`pkg/runtime_helpers/src/memory.rs` — `dart_free` now returns immediately when
`num_bytes == 0`, mirroring the guard `dart_realloc` already had on its allocate path:

```rust
pub extern "C" fn dart_free(ptr: *mut u8, num_bytes: usize, align: usize) {
    // Talc requires a nonzero layout: it documents on `grow`/`shrink` that the
    // caller must ensure the size is greater than zero, and a zero-size
    // `dealloc` sends it a dangling pointer (see `dart_realloc`, which returns
    // `align` for exactly this case). Zero-size blocks were never really
    // allocated, so there is nothing to hand back.
    if num_bytes == 0 {
        return;
    }
    unsafe { dealloc(ptr, Layout::from_size_align_unchecked(num_bytes, align)) }
}
```

**Every caller checked.** `grep -rn "dartFree\|dart_free"` over `pkg/` finds the import
declaration (`pkg/wasm_components/lib/src/embedder/libc.dart:21`), the Rust export, the
Node host's no-op (`tool/run_cases.mjs:933`), and these call sites:

| caller | size argument | can be 0? |
|---|---|---|
| `pkg/wasm_components/lib/src/runtime/string.dart:15` | `2 * packedLength` | **yes** — empty string (the crash above) |
| `pkg/wasm_components/lib/src/embedder/tmp_print.dart:103` (`freeBuffer`) | runtime `totalSize` | **yes** — zero bytes buffered |
| `pkg/wasm_components/lib/src/embedder/clock.dart:37` | `const WasmI32(16)` | no |
| `pkg/wasm_components/lib/src/embedder/tmp_print.dart:220` | `const WasmI32(2)` | no |
| generated `freeBuffer`, template `pkg/wit_bindgen_dart/src/bindgen.rs:189` | `totalSize * size` | **yes** — empty list |
| generated sites: `pkg/wit_bindgen_dart/src/functions.rs:1072`, `:1170`, `src/call_async.rs:42` | generated sizes | **yes** for list-backed ones |
| committed generated files (`pkg/wasi/lib/src/components/*.dart`, `pkg/wasm_tools/example/greeting/...`) | mostly `const WasmI32(n)` struct sizes; list-backed sites use the runtime form | **yes** for the list-backed ones |

Every caller that can pass a zero size reaches it the same way: the matching allocation went
through `mallocAligned` → `dart_realloc(0, 0, align, 0)`, which returns `align` as a dangling
pointer. So `dart_free` was handing talc a zero-size layout and a dangling pointer. The guard
fixes all of them at once; no call site needed changing.

**Unit test** — new `#[cfg(test)] mod tests` in `memory.rs`:

```
$ cargo test --target x86_64-unknown-linux-gnu
running 2 tests
test memory::tests::dart_free_ignores_zero_size_blocks ... ok
test memory::tests::dart_free_still_frees_nonzero_blocks ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

`dart_free_ignores_zero_size_blocks` installs a test-only `#[global_allocator]` that counts
`dealloc` calls, so it fails if a zero-size free ever *reaches* the allocator, not merely if it
happens not to crash. With the guard removed the test binary segfaults — the same fault
`test_runner` hit at `0xfffffffe`. `dart_free_still_frees_nonzero_blocks` guards against the
opposite regression (a free that never frees).

`runtime_helpers.wasm` was rebuilt with the fix
(`pkg/wasm_tools/tool/build_runtime_helpers.sh`, exit 0; asset now 20579 bytes).

### Fix 2 — cause B: the component dropped the SDK's eager static initializers

**The diagnosis above is not the root cause.** Nothing is wrong with `Utf16String`'s stored
length, the `WasmArray<WasmI16>` behind it, or `stringLength` / `stringCodeUnitAt`. The trap
happens on a string the embedder builds itself, before any length crosses the component
boundary — and at index 0.

**Repro.** `String.fromCharCode(0x41)` traps on its own:

```
wasm trap: out of bounds array access
  EmbedderStringImpl.fromCodePoint   (sdk/.../wasm/standalone/embedder_string.dart)
  String.fromCharCode
```

`fromCodePoint` writes into a static buffer before anything else:

```dart
@pragma("wasm:initialize-at-startup")
static final _stringFromCodePointBuffer = WasmArray<WasmI16>(2);

static EmbedderStringImpl fromCodePoint(int codePoint) {
  final array = _stringFromCodePointBuffer;
  array.write(0, codePoint);            // ← traps here, at index 0
  ...
}
```

**Why index 0 traps.** The initializer is not a constant expression, so dart2wasm puts the
field on its *eager* path — the array is allocated in the **module's start function**
(`pkg/dart2wasm/lib/globals.dart:167-186`, `EagerStaticFieldInitializerCodeGenerator`
`.generate(module.startFunction.body, …)`; `_initializeAtStartup` defaults to `true`). Until
that runs, the field holds the *dummy value* dart2wasm uses for a non-nullable reference
global — a **zero-length** array. In the compiled component:

```wat
(global $EmbedderStringImpl._stringFromCodePointBuffer (mut (ref $Array<WasmI16>))
  global.get 775)
(global (;775;) (ref $Array<WasmI16>) array.new_fixed $Array<WasmI16> 0)   ;; ← length 0
```

`array.write(0, …)` on a length-0 array is exactly the observed "out of bounds array access",
even at index 0. No length is wrong anywhere; the array is genuinely empty.

**Root cause.** The start function is replaced during the dart2wasm → component transform and
the SDK's static initializers are lost. The raw dart2wasm module has
`(start $"#func852 #init")`, and `#init` contains
`global.set $EmbedderStringImpl._stringFromCodePointBuffer`. In the component the start
section points at a *new* `$_start` whose body is only

```wat
(func $_start (type 302)
  i32.const 0
  array.new_default $Array<externref>
  call $_invokeMain)
```

`#init` survives as dead code, referenced by nothing. The transform's
`_runMainOnInstantiation` (`pkg/wasm_tools/lib/src/compiler/transform.dart:253-295`) does
handle an existing start — it appends `invokeMain` before the trailing `End` — so it took the
`existingStart == null` branch.

It took that branch because `ModuleTransformer.fromBytes` deserialises the module, and
deserialisation never sets `module.start`. In
`pkg/wasm_tools/lib/src/third_party/wasm_builder/src/ir/module.dart` the field is shadowed by
a parameter of the same name:

```dart
BaseFunction? start;                 // line 27
void initialize(
  ...
  BaseFunction? start,               // shadows the field
  ...
) {
  ...
  start = start;                     // line 65 — assigns the parameter to itself
```

The field stays `null` for **any** module with a start section, on the deserialise path and on
the `ModuleBuilder.forceBuild()` path, which passes `_startFunction` into the same method.
`StartSection.deserialize` is fine — `functions[852]` does resolve to `#init`; the value is
simply never stored. Verified directly: before the fix, deserialising the raw module and
printing `module.start` gives `null`; after, it gives `#init`.

**Fix.** One line:

```diff
-    start = start;
+    this.start = start;
```

This restores the SDK's contract — a module's start function runs at instantiation, so eager
statics are initialised before any exported function is called — instead of papering over the
missing initialisation in the embedder. No bounds check, no `try`/`catch`, no lazily
re-initialising the field on the Dart side. After the fix the component's start function is
`#init` followed by the `invokeMain` call, as intended.

**Strings affected.** Anything whose construction reaches an SDK static that dart2wasm
initialises eagerly: concretely `String.fromCharCode`, `String.fromCodePoint` and
`EmbedderStringImpl.operator[]` (which calls `fromCharCode`). `stringFromCharCodeArray` takes
a different path, which is why `string_from_char_codes` passed tests 0-3 above; the double
cases failed because their formatter indexes result strings. The same missing start function
also left `$_deletedDataMarker` uninitialised — every eager static is restored by this one fix.

### Cause E — a further, unrelated failure that A and B were masking

`double_parse` no longer traps, but it still fails, now on a *different* assertion. Its
`_tryParseExponents` test records a **double**, and the `test_runner` host renders that f64
itself:

| `double.tryParse(...)` | golden | `test_runner` output |
|---|---|---|
| `'1e-3'` | `0.001` | `0.001` ✓ |
| `'0.000001'` | `0.000001` | **`1e-6`** ✗ |
| `'1e23'` | `1e+23` | `1e+23` ✓ |

`main.rs:69` wraps `record-double` as `(f64,)` and `main.rs:130` prints it with
`serde_json::to_string`, so the notation is the host's JSON writer, not Dart's
`double.toString()` — and it disagrees with Dart at small magnitudes. The guest is correct:
`double_to_string` (which formats in the guest and records a *string*) prints `0.000001` for
the same bits, and `run_cases.mjs` passes `double_parse` because its collector formats through
JavaScript's `Number.toString()` (`run_cases.mjs:212-216`), which matches Dart here.

`double_parse_rounding` no longer traps either; it now hits the 30 s `TimeoutException`,
joining `double_parse_rounding_bits` in cause D.

This is a distinct cause, not A or B, so per the task rules it is **reported, not fixed** —
fixing it means changing how the Rust host renders f64, which touches every case that records
a double and needs its own review.

### Verification — `dart test` (the `test_runner` + wasmtime path)

```
cd /workspace/pkg/wasm_components && dart test --reporter expanded
```

Before:

```
07:23 +10 -10: Some tests failed.
=== SUITE EXIT 1 ===
```

After:

```
07:18 +15 -5: Some tests failed.
=== SUITE EXIT 1 at 02:30:37 UTC 2026-09-27 ===
```

| case | before | after |
|---|---|---|
| datetime | pass | pass |
| developer | pass | pass |
| double_parse | FAIL (trap) | **FAIL** — cause E below |
| double_parse_edge | pass | pass |
| double_parse_rounding | FAIL (trap) | **FAIL** — cause D (30 s timeout) |
| double_parse_rounding_bits | FAIL | FAIL — cause D, unchanged |
| double_to_fixed | pass | pass |
| double_to_string | FAIL (trap) | **pass** |
| json_encode_string | pass | pass |
| json_surrogates | pass | pass |
| print | FAIL | FAIL — cause C, unchanged |
| print_astral | FAIL | FAIL — cause C, unchanged |
| regexp | FAIL (trap) | **pass** |
| regexp_classes | pass | pass |
| regexp_semantics | FAIL (trap) | **pass** |
| regexp_unicode | pass | pass |
| string | pass | pass |
| string_from_char_codes | FAIL (trap) | **pass** |
| string_replace | FAIL (trap) | **pass** |
| weak | pass | pass |

**15/20, up from 10/20.** All ten cases that passed before still pass. Newly passing:
`string_from_char_codes`, `string_replace`, `regexp`, `regexp_semantics` (cause A) and
`double_to_string` (cause B).

`double_parse` and `double_parse_rounding` do **not** pass, and that has to be said plainly:
both stopped trapping (their cause-B trap is gone), but each now fails on something else —
cause E and cause D respectively. Neither is a string-length problem, and neither is addressed
by this brief.

### Other suites, no regression

```
pkg/wasm_tools       dart test  → 00:00 +8: All tests passed!   EXIT 0
pkg/runtime_helpers  cargo test → 2 passed; 0 failed            EXIT 0
run_cases.mjs        node       → 20 PASS, 0 FAIL               EXIT 0
```

`run_cases.mjs` never saw cause B because `WebAssembly.instantiate` runs the raw module's
start section, so its eager statics were always initialised; and its `free` is a no-op, so it
never saw cause A either. That is exactly the harness asymmetry described above, and it is
why the two harnesses disagreed.
