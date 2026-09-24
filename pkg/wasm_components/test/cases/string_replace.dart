import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [
    _replaceAllBasics,
    _replaceAllOverlapping,
    _replaceAllEmpty,
    _replaceAllUnicode,
    _replaceRangeBasics,
    _replaceRangeEdges,
    _toCodeUnitsBasics,
    _toCodeUnitsEmpty,
  ]);
}

void _replaceAllBasics(BaseResultCollector collector) {
  collector.recordString(e: 'hello world'.replaceAll('o', '0'));
  collector.recordString(e: 'hello'.replaceAll('l', ''));
  collector.recordString(e: 'hello'.replaceAll('ll', 'L'));
  collector.recordString(e: 'aaa'.replaceAll('aa', 'b'));
  collector.recordString(e: 'hello'.replaceAll('x', 'y'));
  collector.recordString(e: 'hello'.replaceAll('hello', 'bye'));
  collector.recordString(e: ''.replaceAll('a', 'b'));
  collector.recordString(e: 'hello'.replaceAll('', '-'));
}

void _replaceAllOverlapping(BaseResultCollector collector) {
  // Non-overlapping, left to right.
  collector.recordString(e: 'ababab'.replaceAll('abab', 'x'));
  collector.recordString(e: 'aaaa'.replaceAll('aa', 'b'));
  // Replacement containing the needle must not rescan.
  collector.recordString(e: 'ab'.replaceAll('a', 'aa'));
  collector.recordString(e: 'a'.replaceAll('a', 'aaa'));
}

void _replaceAllEmpty(BaseResultCollector collector) {
  collector.recordString(e: 'abc'.replaceAll('abc', ''));
  collector.recordString(e: 'abc'.replaceAll('c', ''));
  // Longer replacement grows the string.
  collector.recordString(e: 'ab'.replaceAll('b', 'xyz'));
}

void _replaceAllUnicode(BaseResultCollector collector) {
  collector.recordString(e: 'aΓbΓc'.replaceAll('Γ', 'g'));
  collector.recordString(e: '𝄞𝄞'.replaceAll('𝄞', '♪'));
  collector.recordString(e: '𐈀x𐈀'.replaceAll('x', '𝄞'));
}

void _replaceRangeBasics(BaseResultCollector collector) {
  collector.recordString(e: 'hello'.replaceRange(1, 3, 'E'));
  collector.recordString(e: 'hello'.replaceRange(0, 5, 'X'));
  collector.recordString(e: 'hello'.replaceRange(2, 2, 'INSERT'));
  collector.recordString(e: 'hello'.replaceRange(0, 0, 'START '));
  collector.recordString(e: 'hello'.replaceRange(5, 5, ' END'));
  collector.recordString(e: 'hello'.replaceRange(0, 2, ''));
}

void _replaceRangeEdges(BaseResultCollector collector) {
  collector.recordString(e: ''.replaceRange(0, 0, 'x'));
  collector.recordString(e: 'abc'.replaceRange(1, 2, '𝄞'));
}

void _toCodeUnitsBasics(BaseResultCollector collector) {
  final a = 'abc'.codeUnits;
  collector.recordInt(e: a.length);
  collector.recordString(e: String.fromCharCodes(a));
  final b = '𝄞'.codeUnits;
  collector.recordInt(e: b.length);
  collector.recordString(e: String.fromCharCodes(b));
  final c = ''.codeUnits;
  collector.recordInt(e: c.length);
}

void _toCodeUnitsEmpty(BaseResultCollector collector) {
  final a = 'héllo'.codeUnits;
  collector.recordInt(e: a.length);
  var sum = 0;
  for (final unit in a) {
    sum += unit;
  }
  collector.recordInt(e: sum);
}
