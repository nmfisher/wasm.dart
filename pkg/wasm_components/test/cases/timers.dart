import 'dart:async';

import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_timers]);
}

Future<void> _timers(BaseResultCollector collector) async {
  final order = <String>[];

  // Root-zone timers: created with `Zone.current == Zone.root`, the SDK's
  // Timer patch routes them through the embedder's scheduleOnce /
  // scheduleRepeated / clearSchedule (the WasmTimer task-zone path is never
  // consulted for these).
  final sw = Stopwatch()..start();
  final one = Completer<void>();
  Zone.root.run(() {
    Timer(const Duration(milliseconds: 30), () {
      order.add('one');
      one.complete();
    });
  });

  // A canceled root-zone timer must never fire.
  Zone.root.run(() {
    Timer(const Duration(milliseconds: 20), () {
      order.add('canceled');
    }).cancel();
  });

  // A periodic root-zone timer fires repeatedly with the requested period
  // until canceled.
  var periodicTicks = 0;
  final periodic = Completer<void>();
  Zone.root.run(() {
    Timer.periodic(const Duration(milliseconds: 10), (timer) {
      periodicTicks++;
      if (periodicTicks == 3) {
        timer.cancel();
        periodic.complete();
      }
    });
  });

  // A timer created in a nested custom zone under the root zone keeps the
  // root-zone (embedder) path: zone delegation walks up to the root.
  final custom = Completer<void>();
  runZoned(
    () {
      Timer(const Duration(milliseconds: 15), () {
        order.add('custom');
        custom.complete();
      });
    },
    zoneSpecification: ZoneSpecification(),
  );

  await Future.wait([one.future, periodic.future, custom.future]);
  sw.stop();

  collector.recordBool(e: order.join(',') == 'custom,one');
  collector.recordBool(e: periodicTicks == 3);
  // The one-shot waited for its full delay (the VM can overshoot, the
  // component's virtual clock cannot fire early either).
  collector.recordBool(e: sw.elapsedMilliseconds >= 30);
  collector.recordInt(e: periodicTicks);
  print('timers ok');
}
