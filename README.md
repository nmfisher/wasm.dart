# Dart WebAssembly components

Tools to compile Dart programs and packages to [WebAssembly components](https://component-model.bytecodealliance.org/),
allowing them to run as CLI applications, servers and more targets with compatible runtimes.

These components can run in `wasmtime run` and `wasmtime serve` without further setup.

> [!WARNING]
> These tools should be considered a proof of concept. They perform very poorly
> due to a lack of GC support in the Component Model.
> A _much_ better way to embed `dart2wasm` into existing applications is to use
> a [custom embedder](https://github.com/simolus3/standalone-embedders).

> [!NOTE]
> Running the compiled components under Wasmtime requires a Dart SDK that emits
> standard Wasm exception instructions (`try_table`): **a Dart SDK
> `^3.14.0-251.0.dev`** (`pkg/wasm_tools` already pins that requirement).
> Use a compatible main-channel SDK or build the SDK from source when the
> released SDKs are older; `pub get` fails on SDK versions below it.
>
> Older dart2wasm emitted the legacy `try` instruction in SDK internals, which
> Wasmtime does not implement, so compiled components validated but could not
> run there. From `3.14.0-251.0.dev` (commit `de942dbb`, *"[dart2wasm]
> Translate exceptions to try_table instructions"*) dart2wasm emits
> `try_table`/`throw_ref` instead, and compiled components run under Wasmtime
> with no special flags. Verified with Wasmtime 47.0.4: `wasmtime run` on
> `pkg/wasm_tools/example/hello_world_wasi` prints the expected output and
> exits 0.

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
3. `wasi`: Generated bindings and helpers to compile Dart as a WASI (CLI or HTTP service) guest.

## Demo

Go to `pkg/wasm_tools/example/greeting`, run `dart run wasm_tools compile bin/app.dart`. This compiles
`bin/app.dart` to `bin/app.wasm`.

Run `wasmtime run bin/app.wasm` to run the app.

## Status

This is a [full list of host imports](https://github.com/dart-lang/sdk/blob/main/sdk/lib/_internal/wasm/standalone/embedder.dart) we need to implement.

Most Dart SDK features are implemented, missing are:

- Regular expressions:
  - Generally work, but support for named groups is not supported.
  - We only parse a subset of the regular expression syntax.
  - Are implemented in Dart with a backtracking engine trying each position (extremely slow).
    Improvements are not in scope for this prototype.
- Weak references, finalizers and expandos are not supported.
- `dart:developer` is mostly unsupported.

## Component tests

The integration suite compiles `pkg/wasm_components/test/cases/*.dart` into
components, executes them with the Rust/Wasmtime runner, and compares the output
with the Dart VM goldens. It exercises the component linker and the Rust runtime
helper, including its allocator.

With a compatible Dart SDK and Rust/Cargo installed, run from the repository root:

```sh
dart pub get
rustup toolchain install nightly --component rust-src
./pkg/wasm_tools/tool/build_runtime_helpers.sh
cd pkg/wasm_components
dart test test/component_test.dart --reporter expanded --timeout 5m
```

`pkg/wasm_components/tool/run_cases.mjs` remains useful as a Rust-free way to
run the case suite in Node.
