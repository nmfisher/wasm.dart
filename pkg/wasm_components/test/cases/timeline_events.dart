import 'dart:developer';

import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_streamEnabledSemantics, _eventShapes]);
}

void _streamEnabledSemantics(BaseResultCollector collector) {
  // With the stream on, timeSync runs the body and returns its value (with
  // the stream off the whole operation collapses to start/finish
  // bookkeeping, but the body runs either way - what the enabled flag
  // changes is the events the SDK reports, which the hosts check on their
  // own sink).
  collector.recordBool(e: Timeline.timeSync('op', () => true));

  // Pinned ids are handed out by the SDK itself, so pass() yields exactly
  // the id the task was created with, as often as it is asked.
  final pinned = TimelineTask.withTaskId(7)..start('render');
  var threwWhileOpen = false;
  try {
    pinned.pass();
  } on StateError {
    threwWhileOpen = true;
  }
  collector.recordBool(e: threwWhileOpen);
  pinned.finish();
  collector.recordBool(e: pinned.pass() == 7);
  collector.recordBool(e: pinned.pass() == 7);

  // start() pushes a block, instant() reports without pushing and finish()
  // pops one, so a second finish has nothing left to pop.
  final task = TimelineTask()
    ..start('a')
    ..instant('b');
  var unevenThrew = false;
  try {
    task.finish();
    task.finish();
  } on StateError {
    unevenThrew = true;
  }
  collector.recordBool(e: unevenThrew);
}

void _eventShapes(BaseResultCollector collector) {
  // The events all of this emits are visible only on the host sink (one
  // NDJSON line per event on the hosts' stderr); the recorded stdout stays
  // identical across hosts. The shapes exercised here are the ones the
  // hosts' checks look for: a begin/end pair with JSON arguments, an
  // instant inside it, and the async begin/instant/end trio of a task.
  Timeline.startSync('bake', arguments: {'kind': 'sour'});
  Timeline.instantSync('moment', arguments: {'at': 'start'});
  Timeline.finishSync();

  final task = TimelineTask()
    ..start('task-phase', arguments: {'frame': 1})
    ..instant('task-moment');
  task.finish(arguments: {'ok': true});
  collector.recordBool(e: task.pass() != 0);
}
