import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_repro]);
}

void _repro(BaseResultCollector collector) {
  // Exponent clamping must consider the digits, not just the digit count:
  // 1e000000001 is 10, not infinity.
  collector.recordString(e: '${double.tryParse('1e000000001')}');
  // Saturation must not apply the sign twice: 1e-9999999999 underflows to 0.
  collector.recordString(e: '${double.tryParse('1e-9999999999')}');
  collector.recordString(e: '${double.tryParse('1e+9999999999')}');
  collector.recordString(e: '${double.tryParse('1e-999999999')}');
  collector.recordString(e: '${double.tryParse('1e00000001')}');
  // A trailing dot with trailing whitespace parses like '1.'.
  collector.recordString(e: '${double.tryParse('1. ')}');
  collector.recordString(e: '${double.tryParse(' 1. ')}');
  collector.recordString(e: '${double.tryParse('1.e5')}');
  // These must keep failing (whole-string rule).
  collector.recordString(e: '${double.tryParse('1.5. ')}');
  collector.recordString(e: '${double.tryParse('1.x')}');
  collector.recordString(e: '${double.tryParse('.')}');
  collector.recordString(e: '${double.tryParse('1. ')}');
}
