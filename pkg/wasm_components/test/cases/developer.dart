import 'dart:developer';

import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_inspectStub, _timelineStub]);
}

void _inspectStub(BaseResultCollector collector) {
  final object = Object();
  collector.recordBool(e: identical(inspect(object), object));
  collector.recordBool(e: inspect(null) == null);
  collector.recordString(e: inspect('echo') as String);
}

void _timelineStub(BaseResultCollector collector) {
  // Time-line helpers are no-ops without a listening stream; the start/
  // finish pair has to stay balanced.
  Timeline.startSync('phase');
  Timeline.instantSync('moment');
  Timeline.finishSync();
  collector.recordBool(e: true);
}
