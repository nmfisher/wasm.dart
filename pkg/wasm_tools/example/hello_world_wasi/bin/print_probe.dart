import 'package:wasi/cli.dart';
import 'package:wasi/cli/command.dart';
import 'package:wasm_components/wasm_components.dart';

void main() {
  commandComponent((imports) => _PrintCommand());
}

final class _PrintCommand implements Run {
  @override
  Future<Result<void, void>> run() async {
    print('first');
    print('');
    print('Dart: 😀 café');
    // Exceed a small host buffer and verify subsequent lines stay ordered.
    print('x' * (128 * 1024));
    print('last');
    // No sleep or explicit flush: command completion must drain print output.
    return const Result.ok(null);
  }
}
