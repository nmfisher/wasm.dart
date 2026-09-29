# Dart WebAssembly components

Tools to compile Dart programs and packages to [WebAssembly components](https://component-model.bytecodealliance.org/),
allowing them to run as CLI applications, servers and more targets with compatible runtimes.

The goal is to get `wasmtime run dart_compiled_app.wasm` to work without further setup.

> [!NOTE]
> These tools are still in development and not ready for production. Please file issues for problems you run into!
>
> **Requires a Dart SDK `^3.14.0-251.0.dev`.** Use a compatible main-channel SDK
> or build the SDK from source when the released SDKs are older.
> `pub get` fails on SDK versions below this requirement.
>
> Older dart2wasm emitted the legacy `try` instruction in SDK internals, which Wasmtime does not
> implement, so compiled components validated but could not run there. From `3.14.0-251.0.dev`
> (commit `de942dbb`, *"[dart2wasm] Translate exceptions to try_table instructions"*) dart2wasm
> emits `try_table`/`throw_ref` instead, and compiled components run under Wasmtime with no
> special flags **on a host that links the `wasm:dart/trace@1.0.0` stack-trace dependency every
> component carries** (verified with Wasmtime 47.0.2 in the case suite's Rust runner). A stock
> `wasmtime run` serves WASI only and rejects the component: dart2wasm keeps the SDK's
> `dart.stackTrace*` imports reachable from core error paths (`Error._throw`, type checks), so
> the compiler cannot stub the dependency away for apps that never read a [StackTrace] - the
> stub path in the transform exists but never fires. A standalone command runner linking the
> `wasm:dart/*` interfaces is follow-up work.

## Approach

Via the experimental `--standalone` mode, Dart can compile to WebAssembly with Dart-specific, documented [host imports](https://github.com/dart-lang/sdk/blob/main/sdk/lib/_internal/wasm/standalone/embedder.dart).
The goal of this project is to:

1. Implement these host imports in WebAssembly, and link their definition into compiled Dart apps to remove the host import.
2. Track metadata around the compilation to lift `dart2wasm` modules into WebAssembly components.

Wherever reasonable, we want to write the WebAssembly implementation for step 1 in Dart. We do this by annotating methods with `@pragma('wasm:export')` and
then use a post-compilation link step that resolves imports against these definitions of the same module.

The main packages in this repository are:

1. `wasm_tools`: CLI tools to generate wit bindings from Dart and to compile Dart to components.
2. `wasm_components`: Dart runtime for the component model.

## Demo

Go to `pkg/wasm_tools/example/hello_world_wasi`, run `dart run wasm_tools compile bin/app.dart`.
This compiles `bin/app.dart` to `bin/app.wasm`.

`bin/app.wasm` imports the `wasm:dart/trace@1.0.0` stack-trace dependency every component
carries (see the note at the top), so run it with a host that links the `wasm:dart/*`
interfaces - the case suite's Rust runner (`pkg/test_runner`) hosts the case components; a
standalone runner for `wasmtime run`-style execution of arbitrary apps is follow-up work.

For a component with a custom world (wit bindings and a link hook), see
`pkg/wasm_tools/example/greeting`; for serving HTTP, `pkg/wasm_tools/example/http_service`.

## Known limitations

Wasmtime 47 enables the Wasm GC and exceptions proposals by default. GC types still
cannot cross a component boundary, though: neither Wasmtime nor this repo's runtime
implements GC integration with the component model, which the Bytecode Alliance names
as the next functionality milestone, to be prototyped on "lazy value lowering"
(["GC and Exceptions in Wasmtime"](https://bytecodealliance.org/articles/wasmtime-gc)).
The interfaces this repo builds therefore speak the usual component-model types -
strings lower to pointers and lengths into a linear memory - and a Dart component
cannot hand a live Dart object to another component; data crossing an interface has
to be marshalled, which is why components still carry a linear memory. Inside a
component this does not bite: host imports use `externref` handles, so the Dart SDK
module and the linked Dart embedder pass objects as opaque references. That is
pinned to Wasmtime 47; a later release implementing lazy value lowering could lift it.

The timezone imports are a runtime gap, not a dead end. The guest carries no timezone
database, and no host serves it one: wasmtime's `wasmtime-wasi` clocks host implements
just the wall/system and monotonic clocks, and this repo's Rust test runner adds no
timezone host call either, so the name always reports `UTC` and the offset a constant
`0`. Three routes out of that gap:

- **Implement the interface in the host (necessary, not sufficient on its own).** The
  compiler-side mapping
  already exists, and it is upstream's, not ours: the `wasi_iana_id` case in
  `pkg/wasm_tools/lib/src/compiler/transform.dart` holds the mapping from the guest's
  timezone-name lookup to `wasi:clocks/timezone@0.3.0` `iana-id` (the same code ships on
  `origin/main`), but the link never fires in this build - this project resolves the
  `dart.*` imports to embedder exports before the transform looks (see the timezone
  attempt below). The interface itself is defined in the WIT wasmtime distributes
  (`iana-id`, `utc-offset`, `to-debug-string`), still marked unstable in the WASI spec -
  so a host implementation would serve an unstable interface. The host half could live
  in either place: in `wasmtime-wasi`, where it would benefit every host, or in this
  repo's runner - the cheaper change, but it serves only hosts using that runner. That
  is the same kind of change as the host-side additions wasmtime merged for
  `wasmtime serve` (#14294, #14320, #14390, #14392, September 2026), and this repository
  already carries a patched dependency (`pkg/wasm_tools/assets/wasm_builder.patch`), so
  carrying a runtime patch fits how the project works.
- **Carry a timezone database in the guest.** Possible, but it means shipping megabytes of
  zone data inside every component, deciding how a zone is selected, and keeping it current
  as rules change.
- **What ships today.** The name reports `UTC` and the offset `0`; the two agree with each
  other, which is what makes the fallback honest rather than misleading - a name like
  `UNKNOWN TZ` would contradict the offset reported beside it.

- **We tried the host half, and it is not enough by itself.** An implementation was written
  for this repository's Rust runner: `wasi:clocks/timezone@0.3.0` with `iana-id` and
  `utc-offset`, the zone taken from `TZ` or the system zone, and the rules read from the
  system `zoneinfo` database. The `jiff` crate it needs is already in this workspace's
  `Cargo.lock`, so no new dependency and no network access were required. It compiled and
  loaded, but a component built from a program that reads `DateTime.now().timeZoneName`
  does not import that interface at all, so the instance had nothing to serve and the
  guest kept reporting the embedder's constant `UTC`. The change was reverted rather than
  kept as dead code, and it only becomes worth having once the guest opts in to asking the
  host, with the trade-off described above.

### The timezone host attempt

Serving `wasi:clocks/timezone@0.3.0` from this repo's Rust runner (`pkg/test_runner`),
next to the existing custom instances for `wasm:dart/trace@1.0.0` and
`wasm:dart/timeline@1.0.0`, was tried and reverted: the runner registered `iana-id` and
`utc-offset`, with the zone taken from `TZ` or the system zone and the rules read from
the system `zoneinfo` database. A component compiled from a program that reads
`DateTime.now().timeZoneName` does not import `wasi:clocks/timezone` at all - its whole
import list is `wasmdart:tests/result-collector`, `wasm:dart/trace@1.0.0`,
`wasi:clocks/system-clock@0.3.0`, `wasi:cli/types@0.3.0`, `wasi:cli/stdout@0.3.0`,
`wasi:clocks/monotonic-clock@0.3.0` - so the instance had nothing to serve. The reason
is the guard on the transform's `wasi_iana_id` case: it links the host interface only
when the module still imports `dart.timeZoneNameForClampedSeconds`, and this project's
design resolves those `dart.*` imports to Dart embedder exports before the transform
looks. The guest reads the embedder's constant instead. Real timezone support therefore
needs a guest-side opt-in - the embedder's two exports asking a host, plus `iana-id`
and `utc-offset` wired in the transform. That would make any component that reads a
zone name require a host that serves the interface, which stock `wasmtime run` cannot,
exactly as `wasm:dart/trace@1.0.0` already does for stack traces. That trade-off is the
open decision, and it is not taken here. Left for follow-up: the module keeps reporting
`UTC` with an offset of `0`, and those two agree.

## Status

This is a [full list of host imports](https://github.com/dart-lang/sdk/blob/main/sdk/lib/_internal/wasm/standalone/embedder.dart) we need to implement.

__Legend__:

- 🎯: This can reasonably be implemented in Dart/WebAssembly without host imports.
- 📦: This requires a host side: a WASI proposal or a component-level host import (e.g. `wasm:dart/trace@1.0.0`).
- 🛑: This is fundamentally unavailable and we can only provide stub imports. The note names the blocker.

| Method                                   | Implemented | Category | Notes                         |
|------------------------------------------|-------------|----------|-------------------------------|
| scheduleOnce                             | ✅          | 📦        | Implemented on WASI timers    |
| scheduleRepeated                         | ✅          | 📦        | Implemented on WASI timers    |
| queueMicrotask                           | ✅          | 🎯        |                               |
| clearSchedule                            | ✅          | 📦        | Implemented on WASI timers    |
| currentTimeMicros                        | ✅          | 📦        |                               |
| stringFromCharCodeArray                  | ✅          | 🎯        |                               |
| stringFromAsciiBytes                     | ✅          | 🎯        |                               |
| stringLength                             | ✅          | 🎯        |                               |
| stringEquals                             | ✅          | 🎯        |                               |
| stringCompare                            | ✅          | 🎯        |                               |
| stringCodeUnitAt                         | ✅          | 🎯        |                               |
| stringIndexOfString                      | ✅          | 🎯        |                               |
| stringLastIndexOfString                  | ✅          | 🎯        |                               |
| stringReplaceAllString                   | ✅          | 🎯        |                               |
| stringReplaceAllRegExp                   | ✅          | 🎯        |                               |
| stringSubstring                          | ✅          | 🎯        |                               |
| stringToLowerCase                        | ✅          | 🎯        |                               |
| stringToUpperCase                        | ✅          | 🎯        |                               |
| stringConcat                             | ✅          | 🎯        |                               |
| stringRepeat                             | ✅          | 🎯        |                               |
| stringReplaceRange                       | ✅          | 🎯        |                               |
| stringToCodeUnits                        | ✅          | 🎯        |                               |
| monotonicClockFrequency                  | ✅          | 📦        |                               |
| monotonicClockTicks                      | ✅          | 📦        |                               |
| weakRefCreate                            | ✅          | 🛑        | Stub: holds the target strongly; blocker: wasm GC gives the embedder no weak references |
| weakRefGet                               | ✅          | 🛑        | Stub: reads the strong ref; blocker: wasm GC gives the embedder no weak references |
| expandoCreate                            | ✅          | 🛑        | Stub: list-backed; blocker: without weak identity, keyed entries leak their targets |
| expandoGet                               | ✅          | 🛑        | Stub: list-backed; blocker: without weak identity, keyed entries leak their targets |
| expandoSet                               | ✅          | 🛑        | Stub: list-backed; blocker: without weak identity, keyed entries leak their targets |
| finalizerCreate                          | ✅          | 🛑        | No-op stub; blocker: wasm GC has no finalization callback the embedder could attach |
| finalizerAttach                          | ✅          | 🛑        | No-op stub; blocker: wasm GC has no finalization callback the embedder could attach |
| finalizerDetach                          | ✅          | 🛑        | No-op stub; blocker: wasm GC has no finalization callback the embedder could attach |
| baseUri                                  | ✅          | 🎯        | Build-time baked `file:` URL of the entry point's directory (`-Ddart.wasm.baseUri`); no host import |
| isWindows                                | ✅          | 🎯        | Build-time baked host platform (`-Ddart.wasm.isWindows`); no host import |
| stackTraceGetCurrent                     | ✅          | 📦        | Frames captured by the host (`wasm:dart/trace@1.0.0` `capture-utf16`) |
| stackTraceToString                       | ✅          | 📦        | Host-rendered trace text via `wasm:dart/trace@1.0.0` |
| doubleTryParse                           | ✅          | 🎯        |                               |
| tryParseResultGetDouble                  | ✅          | 🎯        |                               |
| doubleParseInfallible                    | ✅          | 🎯        |                               |
| i64ToString                              | ✅          | 🎯        | Needs optimization for base10 |
| f64ToExponential                         |             | 🎯        |                               |
| f64ToExponentialWithFractionDigits       |             | 🎯        |                               |
| f64ToPrecision                           |             | 🎯        |                               |
| f64ToFixed                               |             | 🎯        |                               |
| f64ToString                              | ✅          | 🎯        |                               |
| stringBufferCreate                       | ✅          | 🎯        |                               |
| stringBufferWriteString                  | ✅          | 🎯        |                               |
| stringBufferWriteCharCode                | ✅          | 🎯        |                               |
| stringBufferClear                        | ✅          | 🎯        |                               |
| stringBufferLength                       | ✅          | 🎯        |                               |
| stringBufferToString                     | ✅          | 🎯        |                               |
| regexpCreateOrFailWithString             | ✅          | 🎯        | Using `regex` crate in Rust.  |
| regexpIsRegexp                           | ✅          | 🎯        | Using `regex` crate in Rust.  |
| regexpEscape                             | ✅          | 🎯        |                               |
| regexpMatch                              | ✅          | 🎯        | Using `regex` crate in Rust.  |
| regexpMatchGetStart                      | ✅          | 🎯        | Using `regex` crate in Rust.  |
| regexpMatchGetEnd                        | ✅          | 🎯        | Using `regex` crate in Rust.  |
| regexpMatchGetGroupCount                 | ✅          | 🎯        | Using `regex` crate in Rust.  |
| regexpMatchGetGroup                      | ✅          | 🎯        | Using `regex` crate in Rust.  |
| regexpMatchGetNamedGroups                | ✅          | 🎯        | Using `regex` crate in Rust.  |
| regexpMatchGetGroupName                  | ✅          | 🎯        | Using `regex` crate in Rust.  |
| regexpMatchGetGroupByName                | ✅          | 🎯        | Using `regex` crate in Rust.  |
| timeZoneNameForClampedSeconds            | ✅          | 🛑        | Always `UTC` (stub); blocker: no tz db in the guest, and wasmtime's `wasi:clocks/timezone` has no host implementation; see _Known limitations_ |
| timeZoneOffsetInSecondsForClampedSeconds | ✅          | 🛑        | Always 0 (stub); blocker: no tz db in the guest, and wasmtime's `wasi:clocks/timezone` has no host implementation; see _Known limitations_ |
| mathPow                                  | ✅          | 🎯        | via the linked `runtime_helpers` wasm module (`libm`) |
| mathAtan2                                | ✅          | 🎯        | via the linked `runtime_helpers` wasm module (`libm`) |
| mathSin                                  | ✅          | 🎯        | via the linked `runtime_helpers` wasm module (`libm`) |
| mathCos                                  | ✅          | 🎯        | via the linked `runtime_helpers` wasm module (`libm`) |
| mathTan                                  | ✅          | 🎯        | via the linked `runtime_helpers` wasm module (`libm`) |
| mathAcos                                 | ✅          | 🎯        | via the linked `runtime_helpers` wasm module (`libm`) |
| mathAsin                                 | ✅          | 🎯        | via the linked `runtime_helpers` wasm module (`libm`) |
| mathAtan                                 | ✅          | 🎯        | via the linked `runtime_helpers` wasm module (`libm`) |
| mathExp                                  | ✅          | 🎯        | via the linked `runtime_helpers` wasm module (`libm`) |
| mathLog                                  | ✅          | 🎯        | via the linked `runtime_helpers` wasm module (`libm`) |
| randomInt                                | ✅          | 📦        | `wasi:random/insecure` in components; deterministic xorshift in raw modules |
| randomIntSecure                          | ✅          | 📦        | `wasi:random/random` in components; throws where no secure source exists |
| print                                    | ✅          | 📦        | Via `wasi:cli/stdout`; verified in the case suite's Rust runner |
| jsonEncodeString                         | ✅          | 🎯        | JSON string escaping          |
| debugger                                 | ✅          | 🛑        | No-op stub; blocker: no debugger protocol exists for standalone components |
| inspect                                  | ✅          | 🛑        | No-op stub; blocker: no debugger protocol exists for standalone components |
| dartTimelineStreamEnabled                | ✅          | 📦        | On; events go to a host sink (`wasm:dart/timeline@1.0.0` `record-task-event`) |
| reportTaskEvent                          | ✅          | 📦        | One NDJSON line per event on the host's stderr via `wasm:dart/timeline@1.0.0` |

## Component tests

The integration suite compiles `test/cases/*.dart` into components, executes them
with the Rust/Wasmtime runner, and compares the output with Dart VM goldens.
It exercises the component linker and the Rust runtime helper, including Talc.

With a compatible Dart SDK and Rust/Cargo installed, run from the repository root:

```sh
dart pub get
rustup toolchain install nightly --component rust-src
./pkg/wasm_tools/tool/build_runtime_helpers.sh
cd pkg/wasm_components
dart test test/component_test.dart --reporter expanded
```

The helper build needs nightly Rust. The Dart SDK must support the standalone
embedder APIs used by this checkout; Wasmtime execution also requires dart2wasm
to emit standard exception instructions (`try_table`), available from
`3.14.0-251.0.dev`. A sufficiently recent main-channel SDK can be used when the
released SDKs are older.

## Print completion regression test

With the Rust helper built and a compatible Dart SDK, install Wasmtime 47.0.4
and run `dart test test/cli_print_test.dart` from `pkg/wasm_components`.
Set `WASMTIME` to the executable path if it is not on `PATH`.
This test runs a command through the standard Wasmtime CLI and checks that all
print output arrives in order before exit, without an explicit application flush.
