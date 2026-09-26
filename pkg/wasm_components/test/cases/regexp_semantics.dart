import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_repro]);
}

void _repro(BaseResultCollector collector) {
  // P1: repetitions of an atom that can match empty must not recurse forever.
  collector.recordBool(e: RegExp('(?:)*').hasMatch(''));
  collector.recordBool(e: RegExp('(?:)*').hasMatch('x'));
  collector.recordBool(e: RegExp('(a*)*').hasMatch(''));
  collector.recordBool(e: RegExp('(a*)*').hasMatch('a'));
  collector.recordBool(e: RegExp('(?:a?)*b').hasMatch('aab'));
  collector.recordBool(e: RegExp('(?:)+?').hasMatch(''));
  collector.recordBool(e: RegExp('(a*)+?').hasMatch('b'));

  // P1: capture spans must not be truncated at offset 4096, and an empty
  // capture at offset 0 must not read as "did not participate".
  collector.recordString(
    e: RegExp('(a*)').firstMatch('')![1] ?? '<null>',
  );
  final big = RegExp('(a)').firstMatch('b' * 4095 + 'a')!;
  collector.recordInt(e: big.end);
  collector.recordString(e: big[1] ?? '<null>');

  // Stale captures across repetitions and failed lookahead alternatives.
  void groups(String src, String input) {
    final m = RegExp(src).firstMatch(input);
    collector.recordString(
      e: m == null
          ? 'null'
          : [for (var i = 1; i <= m.groupCount; i++) m[i] ?? '-'].join(','),
    );
  }
  groups(r'(a|(b))+', 'aba');
  groups(r'((a)|b)*', 'ab');
  groups(r'(?:(?=(a))b|a)', 'a');
  groups(r'(a*)*', 'aaa');
  groups(r'(a*)+?', 'b');
  groups(r'(a|){2}', 'a');
  groups(r'(?:(a)|){2}', '');
  groups(r'(x(y)?)+', 'x');
  groups(r'(?=(aa|a))a\1', 'aaa');

  // Prefix matching: the supplied start must not become a `^` anchor.
  collector.recordString(
    e: RegExp('^b').matchAsPrefix('ab', 1)?[0] ?? 'null',
  );
  collector.recordString(
    e: RegExp('b').matchAsPrefix('ab', 1)?[0] ?? 'null',
  );
}
