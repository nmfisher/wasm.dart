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
  // Timeline helpers are no-ops without a listening stream; the start/
  // finish bookkeeping still has to stay balanced. Only runtime-independent
  // structural contracts are asserted: task ids are handed out as garbage,
  // sign-flipping values on the VM but sequentially from 1 in the wasm
  // runtime, so id values are never compared.
  Timeline.startSync('phase');
  Timeline.instantSync('moment');
  Timeline.finishSync();
  collector.recordBool(e: true);

  final task = TimelineTask();
  // start() pushes a block (a null placeholder when the stream is disabled),
  // so pass() must refuse while an operation is still open...
  task.start('task-phase');
  var threwWhileOpen = false;
  try {
    task.pass();
  } on StateError {
    threwWhileOpen = true;
  }
  collector.recordBool(e: threwWhileOpen);
  // ...and finish() pops the block again, so passing works afterwards and
  // yields the same stable id every time.
  task.finish();
  collector.recordBool(e: task.pass() == task.pass());

  // finish() without a matching start() is an unbalanced pair.
  var unevenThrows = false;
  try {
    TimelineTask().finish();
  } on StateError {
    unevenThrows = true;
  }
  collector.recordBool(e: unevenThrows);

  // instant() is dropped while the stream is disabled; it must not disturb
  // the start/finish balance.
  final task2 = TimelineTask()
    ..start('task-phase-2')
    ..instant('task-moment');
  task2.finish();
  collector.recordBool(e: true);
}
