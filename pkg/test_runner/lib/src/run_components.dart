import 'components/wasmdart_tests.dart';
import 'components/wasmdart_tests_root.dart';
import 'package:wasm_components/wasm_components.dart';
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
  void invokeTest({required int number}) {
    // A test case is an async-lifted export in its own right: it runs inside
    // a task so that Dart-level async work (streams and futures backed by
    // component-model waitables) can make progress. `print` in particular
    // lowers to a `wasi:cli/stdout` write that completes asynchronously, and
    // without a task there is no event loop to drive it.
    spawnTask(
      debugName: 'invoke-test',
      run: () => cases[number](_collector),
    );
  }
}
