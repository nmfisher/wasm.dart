import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_repro]);
}

void _repro(BaseResultCollector collector) {
  // Surrogate pairs must encode as one 4-byte UTF-8 sequence, and unpaired
  // surrogates as U+FFFD, so printed astral characters survive the byte
  // round-trip through stdout.
  print('emoji: \u{1F600}');
  print('supplementary: \u{10400}\u{1D11E}');
  print('unpaired high: \uD800');
  print('unpaired low: \uDC00');
  print('mixed: a\u{1F600}b\uD800c');
}
