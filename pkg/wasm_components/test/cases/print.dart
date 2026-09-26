import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_printString]);
}

void _printString(BaseResultCollector collector) {
  // print lowers to a wasi:cli/stdout write-via-stream, so this only works
  // when the host runs the component-model event loop.
  collector.recordBool(e: true);
  print('hello from wasm');
  // A second line proves the stream protocol is reusable, not a one-shot.
  print('second line');
}
