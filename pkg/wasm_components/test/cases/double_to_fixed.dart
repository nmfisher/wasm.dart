import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_repro]);
}

void _repro(BaseResultCollector collector) {
  // toStringAsFixed(0) must not leave a trailing decimal point.
  collector.recordString(e: (2.5).toStringAsFixed(0));
  collector.recordString(e: (0.5).toStringAsFixed(0));
  collector.recordString(e: (-2.5).toStringAsFixed(0));
  collector.recordString(e: (1.0).toStringAsFixed(0));
  collector.recordString(e: (0.0).toStringAsFixed(0));
  collector.recordString(e: (-0.5).toStringAsFixed(0));
  collector.recordString(e: (123.456).toStringAsFixed(0));
  // Larger inputs keep the fraction point.
  collector.recordString(e: (2.5).toStringAsFixed(2));
  collector.recordString(e: (2.5).toStringAsFixed(1));
  // A rounding rollover grows into a new integer digit.
  collector.recordString(e: (9.99).toStringAsFixed(0));
  collector.recordString(e: (0.99).toStringAsFixed(0));
  collector.recordString(e: (0.099).toStringAsFixed(1));
}
