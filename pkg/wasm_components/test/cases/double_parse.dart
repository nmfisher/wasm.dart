import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [
    _tryParseBasics,
    _tryParseWhitespace,
    _tryParseExponents,
    _tryParseSpecials,
    _tryParseFailures,
    _infallibleJsonNumbers,
    _roundTrip,
  ]);
}

void _tryParseBasics(BaseResultCollector collector) {
  collector.recordDouble(e: double.tryParse('0')!);
  collector.recordDouble(e: double.tryParse('1')!);
  collector.recordDouble(e: double.tryParse('-1')!);
  collector.recordDouble(e: double.tryParse('1.5')!);
  collector.recordDouble(e: double.tryParse('-1.5')!);
  collector.recordDouble(e: double.tryParse('0.5')!);
  collector.recordDouble(e: double.tryParse('+2.25')!);
  collector.recordDouble(e: double.tryParse('00123')!);
  collector.recordDouble(e: double.tryParse('12.50')!);
  collector.recordDouble(e: double.tryParse('3.141592653589793')!);
}

void _tryParseWhitespace(BaseResultCollector collector) {
  collector.recordDouble(e: double.tryParse(' 1 ')!);
  collector.recordDouble(e: double.tryParse('\t1.5\n')!);
  collector.recordBool(e: double.tryParse('- 1') == null);
  collector.recordBool(e: double.tryParse('1 . 5') == null);
}

void _tryParseExponents(BaseResultCollector collector) {
  collector.recordDouble(e: double.tryParse('1e3')!);
  collector.recordDouble(e: double.tryParse('1E3')!);
  collector.recordDouble(e: double.tryParse('1e+3')!);
  collector.recordDouble(e: double.tryParse('1e-3')!);
  collector.recordDouble(e: double.tryParse('1.5e-3')!);
  collector.recordDouble(e: double.tryParse('-2.5e2')!);
  collector.recordBool(e: double.tryParse('1e') == null);
  collector.recordDouble(e: double.tryParse('.5e1')!);
  collector.recordDouble(e: double.tryParse('0.000001')!);
  collector.recordDouble(e: double.tryParse('1e23')!);
  // Overflows to infinity (recorded as finiteness; the collector cannot
  // encode infinities as JSON).
  collector.recordBool(e: double.tryParse('1e309')!.isInfinite);
  collector.recordBool(e: double.tryParse('-1e309')!.isInfinite);
  collector.recordBool(e: double.tryParse('-1e309')!.isNegative);
  // Underflows to zero.
  collector.recordBool(e: double.tryParse('1e-330') == 0.0);
}

void _tryParseSpecials(BaseResultCollector collector) {
  final nan = double.tryParse('NaN')!;
  collector.recordBool(e: nan.isNaN);
  // Infinities can not be JSON-encoded; record their finiteness/sign.
  collector.recordBool(e: double.tryParse('Infinity')!.isInfinite);
  collector.recordBool(e: double.tryParse('-Infinity')!.isNegative);
  collector.recordBool(e: double.tryParse('inf') == null);
  collector.recordBool(e: double.tryParse('nan') == null);
  collector.recordBool(e: double.tryParse('NaN ')!.isNaN);
  collector.recordBool(e: double.tryParse(' Infinity ')!.isInfinite);
}

void _tryParseFailures(BaseResultCollector collector) {
  collector.recordBool(e: double.tryParse('') == null);
  collector.recordBool(e: double.tryParse(' ') == null);
  collector.recordBool(e: double.tryParse('.') == null);
  collector.recordBool(e: double.tryParse('1.2.3') == null);
  collector.recordBool(e: double.tryParse('abc') == null);
  collector.recordBool(e: double.tryParse('1x') == null);
  collector.recordBool(e: double.tryParse('e5') == null);
  collector.recordBool(e: double.tryParse('-') == null);
  collector.recordBool(e: double.tryParse('1e5.5') == null);
  collector.recordBool(e: double.tryParse('0x10') == null);
  // Trailing dot is fine, but not twice.
  collector.recordDouble(e: double.tryParse('1.')!);
  collector.recordBool(e: double.tryParse('1.5.') == null);
  collector.recordDouble(e: double.tryParse('-1.e3')!);
}

void _infallibleJsonNumbers(BaseResultCollector collector) {
  // double.parse on strings already known to be valid JSON numbers goes
  // through doubleParseInfallible.
  collector.recordDouble(e: double.parse('123.456'));
  collector.recordDouble(e: double.parse('-0.5'));
  collector.recordDouble(e: double.parse('2e10'));
}

void _roundTrip(BaseResultCollector collector) {
  // Parse-back of formatted doubles must be exact.
  for (final s in const [
    '0.1',
    '0.2',
    '1e-7',
    '123.456',
    '1.7976931348623157e308',
    '5e-324',
    '2.2250738585072014e-308',
  ]) {
    final v = double.parse(s);
    collector.recordBool(e: v.toString() == s || double.parse(v.toString()) == v);
  }
}
