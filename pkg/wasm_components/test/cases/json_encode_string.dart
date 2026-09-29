import 'dart:convert';

import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_basics, _escapes, _unicode, _nonStringInputs]);
}

void _basics(BaseResultCollector collector) {
  collector.recordString(e: jsonEncode('hello'));
  collector.recordString(e: jsonEncode(''));
  collector.recordString(e: jsonEncode('a b c'));
}

void _escapes(BaseResultCollector collector) {
  collector.recordString(e: jsonEncode('a"b'));
  collector.recordString(e: jsonEncode(r'a\b'));
  collector.recordString(e: jsonEncode('a\nb\tc'));
  collector.recordString(e: jsonEncode('\x08\x0c\x0d'));
  collector.recordString(e: jsonEncode('\x01\x1f'));
  collector.recordString(e: jsonEncode('\x00'));
}

void _unicode(BaseResultCollector collector) {
  collector.recordString(e: jsonEncode('\x7f'));
  collector.recordString(e: jsonEncode('Γ𝄞'));
  collector.recordString(e: jsonEncode('𝄞'));
  collector.recordString(e: jsonEncode('\uD800')); // lone surrogate
  collector.recordString(e: jsonEncode('\u2028\u2029'));
}

void _nonStringInputs(BaseResultCollector collector) {
  // Public dart:convert coverage; the embedder import is tested separately
  // through Error.safeToString in error_string_encoding.dart.
  collector.recordString(e: jsonEncode(true));
  collector.recordString(e: jsonEncode(null));
  collector.recordString(e: jsonEncode(1.5));
  collector.recordString(e: jsonEncode(12));
}
