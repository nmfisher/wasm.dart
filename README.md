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
> special flags. Verified with Wasmtime 47.0.4: `wasmtime run` on
> `pkg/wasm_tools/example/hello_world_wasi` prints the expected output and exits 0.

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

Go to `examples/hello_world_custom`, run `dart run wasm_tools compile bin/app.dart`. This compiles
`bin/app.dart` to `bin/app.wasm`.

Run `cargo run` to run this app with Wasmtime.

## Status

This is a [full list of host imports](https://github.com/dart-lang/sdk/blob/main/sdk/lib/_internal/wasm/standalone/embedder.dart) we need to implement.

__Legend__:

- 🎯: This can reasonably be implemented in Dart/WebAssembly without host imports.
- 📦: This requires a host import (a WASI proposal).
- 🛑: This is fundamentally unavailable and we can only provide stub imports.

| Method                                   | Implemented | Category | Notes                         |
|------------------------------------------|-------------|----------|-------------------------------|
| scheduleOnce                             | ✅          | 📦        | Stub; throws in raw module    |
| scheduleRepeated                         | ✅          | 📦        | Stub; throws in raw module    |
| queueMicrotask                           | ✅          | 🎯        |                               |
| clearSchedule                            | ✅          | 📦        | Stub; throws in raw module    |
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
| weakRefCreate                            | ✅          | 🛑        | Strong ref stub; no GC yet    |
| weakRefGet                               | ✅          | 🛑        | Strong ref stub; no GC yet    |
| expandoCreate                            | ✅          | 🛑        | Backed by a list; no GC yet   |
| expandoGet                               | ✅          | 🛑        | Backed by a list; no GC yet   |
| expandoSet                               | ✅          | 🛑        | Backed by a list; no GC yet   |
| finalizerCreate                          | ✅          | 🛑        | No-op stub; no GC yet         |
| finalizerAttach                          | ✅          | 🛑        | No-op stub; no GC yet         |
| finalizerDetach                          | ✅          | 🛑        | No-op stub; no GC yet         |
| baseUri                                  | ✅          | 📦        | Fixed file:/// stub           |
| isWindows                                | ✅          | 📦        | Always false                  |
| stackTraceGetCurrent                     | ✅          | 🛑        | Impossible, stub used         |
| stackTraceToString                       | ✅          | 🛑        | Impossible, stub used         |
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
| timeZoneNameForClampedSeconds            |             | 📦        | Unimplemented in wasmtime     |
| timeZoneOffsetInSecondsForClampedSeconds |             | 📦        | Unimplemented in wasmtime     |
| mathPow                                  | ✅          | 🎯        | Using `libm` in Rust.         |
| mathAtan2                                | ✅          | 🎯        | Using `libm` in Rust.         |
| mathSin                                  | ✅          | 🎯        | Using `libm` in Rust.         |
| mathCos                                  | ✅          | 🎯        | Using `libm` in Rust.         |
| mathTan                                  | ✅          | 🎯        | Using `libm` in Rust.         |
| mathAcos                                 | ✅          | 🎯        | Using `libm` in Rust.         |
| mathAsin                                 | ✅          | 🎯        | Using `libm` in Rust.         |
| mathAtan                                 | ✅          | 🎯        | Using `libm` in Rust.         |
| mathExp                                  | ✅          | 🎯        | Using `libm` in Rust.         |
| mathLog                                  | ✅          | 🎯        | Using `libm` in Rust.         |
| randomInt                                | ✅          | 📦        | Deterministic in raw module   |
| randomIntSecure                          | ✅          | 📦        | Throws in raw module          |
| print                                    | ✅          | 📦        | Via `wasi:cli/stdout`; completes under Wasmtime (e.g. `hello_world_wasi`) |
| jsonEncodeString                         | ✅          | 🎯        | JSON string escaping          |

| debugger                                 | ✅          | 🛑        | No-op; no debugger attached   |
| inspect                                  | ✅          | 🛑        | No-op; no debugger attached |
| dartTimelineStreamEnabled                | ✅          | 🛑        | Always false                  |
| reportTaskEvent                          | ✅          | 🛑        | No-op stub; events dropped    |

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
