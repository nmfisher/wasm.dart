import 'dart:io';

import 'package:path/path.dart' as p;
import 'package:test/test.dart';
import 'package:test_descriptor/test_descriptor.dart' as d;

void main() {
  test('print output is complete and ordered when a WASI command returns', () async {
    final component = d.path('print.wasm');
    final compile = await Process.run(Platform.executable, [
      'run',
      'wasm_tools',
      'compile',
      'bin/print_probe.dart',
      '--output',
      component,
    ], workingDirectory: p.normalize('../wasm_tools/example/hello_world_wasi'));
    expect(compile.exitCode, 0, reason: '${compile.stdout}\n${compile.stderr}');

    // Use the standard CLI, with no custom host or stdout-draining workaround.
    final result = await Process.run(
      Platform.environment['WASMTIME'] ?? 'wasmtime',
      ['run', component],
    );
    expect(result.exitCode, 0, reason: '${result.stderr}');
    expect(
      result.stdout,
      'first\n\nDart: 😀 café\n${'x' * (128 * 1024)}\nlast\n',
    );
  }, timeout: const Timeout(Duration(minutes: 2)));
}
