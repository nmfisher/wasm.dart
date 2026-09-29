import 'components/wasmdart_tests.dart';
import 'components/wasmdart_tests_root.dart';
import 'testcase.dart';

void defineTests(List<TestCase> cases) {
  rootComponent((imports) => _ExportTestModule(cases, imports));
}

final class const _ImportedCollector(final ResultCollector collector)
    implements BaseResultCollector {
  @override
  void recordDouble({required double e}) {
    collector.recordDouble(e: e);
  }

  @override
  void recordInt({required int e}) {
    collector.recordInt(e: e);
  }

  @override
  void recordString({required String e}) {
    collector.recordString(e: e);
  }

  @override
  void recordBool({required bool e}) {
    collector.recordBool(e: e);
  }
}

final class _ExportTestModule(final List<TestCase> cases, RootImports imports)
    implements TestedModule {
  final _collector = _ImportedCollector(imports.testsResultCollector);

  @override
  int countTests() => cases.length;

  @override
  Future<void> invokeTest({required int number}) async {
    // The generated export wrapper runs this inside a component-model task
    // (its `spawnTask`), so async work - `print` lowering to a
    // `wasi:cli/stdout` stream write - is driven by the host polling the
    // exported `callback` until the task's waitable set is empty.
    final run = cases[number];
    run(_collector);
  }
}
