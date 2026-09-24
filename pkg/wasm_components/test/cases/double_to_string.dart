import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_wholeNumbers, _fractions, _specialValues, _exponents]);
}

void _wholeNumbers(BaseResultCollector collector) {
  collector.recordString(e: 0.0.toString());
  collector.recordString(e: (-0.0).toString());
  collector.recordString(e: 1.0.toString());
  collector.recordString(e: (-1.0).toString());
  collector.recordString(e: 100.0.toString());
  collector.recordString(e: (-42.0).toString());
}

void _fractions(BaseResultCollector collector) {
  collector.recordString(e: 0.5.toString());
  collector.recordString(e: 1.5.toString());
  collector.recordString(e: 123.456.toString());
  collector.recordString(e: 3.141592653589793.toString());
  collector.recordString(e: 42.5.toString());
}

void _specialValues(BaseResultCollector collector) {
  collector.recordString(e: double.nan.toString());
  collector.recordString(e: double.infinity.toString());
  collector.recordString(e: double.negativeInfinity.toString());
}

void _exponents(BaseResultCollector collector) {
  // The shortest round-trip representation of these values needs an exponent.
  collector.recordString(e: 1e21.toString());
  collector.recordString(e: 1.5e21.toString());
  collector.recordString(e: (-2.5e-8).toString());
  collector.recordString(e: 1.7976931348623157e308.toString());
  collector.recordString(e: 5e-7.toString());
  collector.recordString(e: 1e-6.toString());
}
