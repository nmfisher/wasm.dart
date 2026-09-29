import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_strings, _argumentError]);
}

void _strings(BaseResultCollector collector) {
  for (final value in [
    '',
    'hello',
    'a"b\\c',
    '\b\t\n\f\r',
    '\u0000\u0001\u001f',
    'éΓ',
    'Γ\b\t\n\f\r\u0000\u001f"\\',
    '\u007f\u2028\u2029',
    'a😀b',
    '\ud800',
    '\udfff',
    '\ud800x\udfff',
    '\ud800\ud800\udc00\udc00',
  ]) {
    // Inspect code units so UTF-8 output cannot hide unpaired surrogates.
    final encoded = Error.safeToString(value);
    collector.recordString(e: encoded.codeUnits.join(','));
  }
}

void _argumentError(BaseResultCollector collector) {
  collector.recordString(
    e: ArgumentError.value('first\n"second"', 'input').toString(),
  );
}
