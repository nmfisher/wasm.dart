@internal
library;

// ignore: import_internal_library
import 'dart:_wasm';
import 'dart:async';

import 'package:meta/meta.dart';

import '../../embedder/clock.dart';
import 'subtask.dart';
import 'task.dart';

final class WasmTimer implements Timer {
  final Task _task;
  final Duration _duration;

  /// The callback, which should be bound to the zone creating this timer.
  final void Function() _callback;
  final bool _isPeriodic;

  Subtask? _currentWait;

  @override
  var isActive = true;

  @override
  var tick = 0;

  new({
    required this._task,
    required this._duration,
    required this._callback,
    required this._isPeriodic,
  }) {
    _schedule();
  }

  void _schedule() {
    assert(_currentWait == null && isActive);
    final inNanos = (_duration.inMicroseconds * 1000).toWasmI64();
    final task = _currentWait = _task.trackSubtask(
      wasiMonotonicWaitFor(inNanos),
    );

    task.completion.onError<SubtaskCancelledException>((_, _) {}).whenComplete(
      () {
        if (isActive) {
          if (!_isPeriodic) isActive = false;

          tick++;
          _callback();

          if (_isPeriodic && isActive) {
            _schedule();
          }
        }
      },
    );
  }

  @override
  void cancel() {
    isActive = false;
    _currentWait?.cancel();
    _currentWait = null;
  }
}

/// A timer registered through the embedder's `scheduleOnce` /
/// `scheduleRepeated` exports, i.e. by the SDK's root-zone `Timer` patch.
///
/// Unlike [WasmTimer] this is not bound to the zone that created it: the SDK
/// patch stores only an opaque handle. The callback is kept unbound and is
/// always run in the task the timer was registered in, which is where its
/// continuations must run for the event loop to make progress.
final class EmbedderTimer {
  final Task _task;
  final Duration _duration;
  final void Function() _callback;
  final bool _isPeriodic;

  WasmTimer? _timer;
  var _canceled = false;

  EmbedderTimer(this._task, this._duration, this._callback, this._isPeriodic) {
    // Run inside the task's zone so that the SDK timer machinery and the
    // user callback behave exactly like a timer created within the task.
    // `trackSubtask` requires the thread to be associated with its task and
    // the task to be marked running; timer callbacks fire outside of the
    // event loop (the host calls us directly), so restore that association.
    _task.enter();
    try {
      _task.runInZone(() {
        _timer = WasmTimer(
          task: _task,
          isPeriodic: _isPeriodic,
          duration: _duration,
          callback: () {
            if (!_canceled) _callback();
          },
        );
      });
    } catch (e, s) {
      // TEMP DEBUG
      print('EMB-TIMER ctor error: $e');
      print('$s');
    } finally {
      _task.exit();
    }
  }

  /// Returns whether the timer was still alive (it had not fired yet and had
  /// not been canceled).
  bool cancel() {
    final timer = _timer;
    if (_canceled || timer == null || !timer.isActive) return false;
    _canceled = true;
    timer.cancel();
    return true;
  }
}
