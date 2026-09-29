// ignore_for_file: type=warning

abstract interface class ResultCollector {
  void recordString({required String e});
  void recordDouble({required double e});
  void recordInt({required int e});
  void recordBool({required bool e});
}

abstract interface class TestedModule {
  int countTests();

  /// Runs one test case and waits for pending Dart print writes before
  /// reporting completion to the host.
  Future<void> invokeTest({required int number});
}
