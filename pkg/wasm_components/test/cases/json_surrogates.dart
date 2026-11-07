import 'dart:convert' show jsonEncode;

import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_repro]);
}

void _repro(BaseResultCollector collector) {
  // Lone surrogates must be escaped like dart:convert does (the VM is the
  // golden oracle for this suite), and paired ones pass through.
  collector.recordString(e: jsonEncode('\uD800'));
  collector.recordString(e: jsonEncode('\uDC00'));
  collector.recordString(e: jsonEncode('a\uD800b'));
  collector.recordString(e: jsonEncode('\u{1F600}'));
  collector.recordString(e: jsonEncode('\u2028\u2029'));
  collector.recordString(e: jsonEncode('\x7f'));
  // Pairs pass through, then a lone unit is escaped: distinguishes
  // guest-side escaping from the host JSON.stringify display path.
  collector.recordString(e: jsonEncode('\uD800\uDC00\uD800'));
  collector.recordString(e: jsonEncode('\uD800A\uDC00'));
  collector.recordString(e: jsonEncode('\uDC00\u{1F600}'));
}
