import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_probe]);
}

void _probe(BaseResultCollector collector) {
  collector.recordString(e: 'trace:${StackTrace.current.toString().length}');
  try {
    collector.recordString(e: 'urilen:${Uri.base.toString().length}');
  } catch (e) {
    collector.recordString(e: 'ERR:$e');
  }
}
