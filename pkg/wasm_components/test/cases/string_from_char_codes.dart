import 'dart:convert';
import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_ascii, _latin1, _bmp, _surrogatePairs, _empty, _subRanges]);
}

void _ascii(BaseResultCollector collector) {
  collector.recordString(e: String.fromCharCodes([104, 101, 108, 108, 111]));
}

void _latin1(BaseResultCollector collector) {
  collector.recordString(e: String.fromCharCodes([0xe9, 0xfc, 0xdf]));
}

void _bmp(BaseResultCollector collector) {
  collector.recordString(
    e: String.fromCharCodes([0x1F600 < 0x10000 ? 0x1F600 : 0x1F600]),
  );
}

void _surrogatePairs(BaseResultCollector collector) {
  // The emoji 😀 is stored as a surrogate pair.
  collector.recordString(
    e: String.fromCharCodes([0xd83d, 0xde00]),
  );
  // Astral characters in the middle of a string.
  collector.recordString(e: 'a${String.fromCharCodes([0xd83d, 0xde00])}b');
}

void _empty(BaseResultCollector collector) {
  collector.recordString(e: String.fromCharCodes(<int>[]));
}

void _subRanges(BaseResultCollector collector) {
  final json = const JsonEncoder().convert('irrelevant');
  collector.recordBool(e: json.isNotEmpty);
  collector.recordInt(e: 'a'.codeUnitAt(0));
}
