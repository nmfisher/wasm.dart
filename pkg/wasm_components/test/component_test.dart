import 'dart:io';

import 'package:test/test.dart';
import 'package:path/path.dart' as p;
import 'package:test_descriptor/test_descriptor.dart' as d;

/// Per-case timeouts for the cases that legitimately outlast package:test's
/// 30 s default. Everything not listed here keeps that default.
///
/// Measured on the 4-CPU Linux dev box, running the suite as `dart test` does
/// (one test at a time, warm `wasm_tools` compile cache):
///
/// | case                       | compile | runner | of which JIT+instantiate | case body |
/// |----------------------------|---------|--------|--------------------------|-----------|
/// | print (baseline)           | 3.5 s   | 15.4 s | 15.6 s                   | ~0.02 s   |
/// | double_parse_rounding      | 3.6 s   | 135 s  | 18.5 s                   | 115.7 s   |
/// | double_parse_rounding_bits | 3.6 s   | 30.9 s | 19.2 s                   | 12.1 s    |
///
/// The fixed ~15-19 s is wasmtime JIT-compiling the debug dart2wasm module
/// (~2k functions) through a debug-build cranelift before the guest runs a
/// single step - `print` spends it all and its body is instantaneous. On top
/// of that, these two cases run the guest's BigInt-exact `double.parse`
/// embedder, which is expensive in a debug-built guest: `double_parse_rounding`
/// formats 462 literals and its subnormal range shifts a >1000-bit numerator
/// (`<< 1074`) per literal, which is where its 115.7 s goes.
///
/// This is measured cost, not a hang: with these timeouts the cases finish and
/// match their goldens, and a genuine hang would still fail the test, just
/// later. Making them faster would mean building the runner (and its cranelift)
/// in release mode or rewriting the guest's exact-decimal arithmetic - both
/// changes to what is being tested, not to how long it takes.
const slowCaseTimeouts = <String, Timeout>{
  'double_parse_rounding': Timeout(Duration(minutes: 5)),
  'double_parse_rounding_bits': Timeout(Duration(seconds: 90)),
};

void main() async {
  late String testRunnerExecutable;

  setUpAll(() async {
    final cargoBuild = await Process.start('cargo', [
      'build',
      '-p',
      'test_runner',
    ], mode: .inheritStdio);
    final exitCode = await cargoBuild.exitCode;
    if (exitCode != 0) throw 'Unexpected exit code from cargo: $exitCode';

    final exeSuffix = Platform.isWindows ? '.exe' : '';
    testRunnerExecutable = p.normalize(
      '../../target/debug/test_runner$exeSuffix',
    );
  });

  await for (final file in Directory('test/cases').list()) {
    if (file is! File || p.extension(file.path) != '.dart') continue;

    final name = p.basenameWithoutExtension(file.path);
    test(
      name,
      () async {
        final wasmFile = d.path('test.wasm');
        final result = await Process.run(Platform.executable, [
          'run',
          'wasm_tools',
          'compile',
          file.path,
          '--output',
          wasmFile,
          '--hooks-include-dev-dependencies',
          '--no-implicit-wasi-imports',
        ]);

        if (result.exitCode != 0) {
          throw 'Could not compile: ${result.stdout} ${result.stderr}';
        }

        final output = await Process.run(testRunnerExecutable, [wasmFile]);
        if (output.exitCode != 0) {
          throw 'Could not run test runner: ${output.exitCode}: ${output.stdout} ${output.stderr}';
        }

        final golden = await File(p.setExtension(file.path, '.golden.txt'))
            .readAsString();
        expect(output.stdout, golden);
      },
      timeout: slowCaseTimeouts[name],
    );
  }
}
