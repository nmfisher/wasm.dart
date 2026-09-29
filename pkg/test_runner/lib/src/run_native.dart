import 'dart:async';
import 'dart:convert';

import 'testcase.dart';

void defineTests(List<TestCase> cases) {
  // Case bodies may be async: run them one after another on the VM's event
  // loop. Pending timers and microtasks keep the isolate alive until every
  // case has finished.
  unawaited(_runAll(cases));
}

Future<void> _runAll(List<TestCase> cases) async {
  for (final (idx, run) in cases.indexed) {
    _printJson(serializeTestStart(idx));
    try {
      await run(const _PrintRunner());
    } finally {
      _printJson(serializeTestEnd(idx));
    }
  }
}

void _printJson(Object obj) {
  print(json.encode(obj));
}

final class _PrintRunner implements BaseResultCollector {
  const _PrintRunner();

  @override
  void recordDouble({required double e}) {
    _printJson(serializeRecordedDouble(e));
  }

  @override
  void recordInt({required int e}) {
    _printJson(serializeRecordedInt(e));
  }

  @override
  void recordString({required String e}) {
    _printJson(serializeRecordedString(e));
  }

  @override
  void recordBool({required bool e}) {
    _printJson(serializeRecordedBool(e));
  }
}
