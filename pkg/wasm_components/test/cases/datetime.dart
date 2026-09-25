import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_timeZoneOffset, _utcConstructor]);
}

void _timeZoneOffset(BaseResultCollector collector) {
  // Without a tz db the embedder reports UTC (offset 0) for local times.
  final epoch = DateTime.fromMillisecondsSinceEpoch(0);
  collector.recordBool(e: epoch.timeZoneOffset == Duration.zero);
  // The time zone *name* differs between runtimes (host zone vs the
  // embedder's UNKNOWN TZ id), so it is not recorded here.
  collector.recordBool(e: epoch.timeZoneName.isNotEmpty);
}

void _utcConstructor(BaseResultCollector collector) {
  final utc = DateTime.utc(2026, 9, 25, 12, 30);
  collector.recordInt(e: utc.year);
  collector.recordInt(e: utc.minute);
  collector.recordBool(e: utc.isUtc);
  collector.recordBool(e: utc.timeZoneOffset == Duration.zero);
}
