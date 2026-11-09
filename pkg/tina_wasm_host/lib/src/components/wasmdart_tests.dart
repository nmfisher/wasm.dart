// ignore_for_file: type=warning
import r'package:wasm_components/wasm_components.dart' as i0;

abstract interface class ResultCollector {
  void recordString({required String e});
  void recordOptionalString({required i0.Option<String> e});
  void recordStringList({required List<String> e});
  void recordDouble({required double e});
  void recordInt({required int e});
  void recordBool({required bool e});
}

abstract interface class TestedModule {
  int countTests();

  /// Runs one test case. Async because a case may perform component-model
  /// async work (print lowers to a wasi:cli/stdout stream write): the caller
  /// keeps polling `callback` until the returned task has no waitables left.
  Future<void> invokeTest({required int number});
}
