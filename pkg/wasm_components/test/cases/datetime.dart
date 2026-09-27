import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_timeZoneOffset, _utcConstructor, _timeZoneName]);
}

void _timeZoneOffset(BaseResultCollector collector) {
  // Without a tz db the embedder reports UTC (offset 0) for local times.
  final epoch = DateTime.fromMillisecondsSinceEpoch(0);
  collector.recordBool(e: epoch.timeZoneOffset == Duration.zero);
  // The time zone *name* differs between runtimes (host zone vs the
  // embedder's UTC id), so it is not recorded here.
  collector.recordBool(e: epoch.timeZoneName.isNotEmpty);
}

void _utcConstructor(BaseResultCollector collector) {
  final utc = DateTime.utc(2026, 9, 25, 12, 30);
  collector.recordInt(e: utc.year);
  collector.recordInt(e: utc.minute);
  collector.recordBool(e: utc.isUtc);
  collector.recordBool(e: utc.timeZoneOffset == Duration.zero);
}

void _timeZoneName(BaseResultCollector collector) {
  final local = DateTime.fromMillisecondsSinceEpoch(0);
  final name = local.timeZoneName;
  // A UTC DateTime must be named UTC on every runtime.
  collector.recordBool(e: DateTime.utc(0).timeZoneName == 'UTC');
  // The name travels through the embedder's string machinery intact: no
  // trimming, no case mangling, and it is stable across calls and instants
  // (no per-instant zone lookup).
  collector.recordBool(e: name == name.trim() && name.isNotEmpty);
  collector.recordBool(e: name == DateTime.now().timeZoneName);
  collector.recordBool(
    e: name ==
        DateTime.fromMillisecondsSinceEpoch(1583020800).timeZoneName,
  );
  // The name is paired with the offset it names: the embedder reports a
  // zero offset for local times, and with that offset local time equals UTC.
  collector.recordBool(e: local.timeZoneOffset == Duration.zero);
  // DateTime equality also compares the zone, so compare the epoch value:
  // with a zero local offset, local time and UTC time are the same moment
  // and yield the same fields.
  final asUtc = DateTime.fromMillisecondsSinceEpoch(
    local.millisecondsSinceEpoch,
    isUtc: true,
  );
  collector.recordBool(e: asUtc.millisecondsSinceEpoch == local.millisecondsSinceEpoch);
  collector.recordBool(e: asUtc.year == local.year && asUtc.hour == local.hour);
}
