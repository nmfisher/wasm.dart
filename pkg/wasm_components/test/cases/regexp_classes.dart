import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_repro]);
}

RegExp _rx(String source, {bool i = false, bool u = false}) =>
    RegExp(source, caseSensitive: !i, unicode: u);

/// Finding #13: class escapes inside `[…]`, the empty classes `[]`/`[^]`, and
/// the Annex B fallback when an escape appears next to a range `-`.
void _repro(BaseResultCollector collector) {
  // Class escapes are ordinary members.
  collector.recordString(e: _rx(r'[\d]+').firstMatch('a1')![0]!);
  collector.recordString(e: _rx(r'[\D]+').firstMatch('1a')![0]!);
  collector.recordString(e: _rx(r'[\w]+').firstMatch('ab_1!')![0]!);
  collector.recordString(e: _rx(r'[\W]+').firstMatch('ab!')![0]!);
  collector.recordString(e: _rx(r'[\s]+').firstMatch('a \t')![0]!);
  collector.recordString(e: _rx(r'[\S]+').firstMatch('a b')![0]!);
  collector.recordString(e: _rx(r'[\d\s]+').firstMatch('x1 y')![0]!);
  collector.recordString(e: _rx(r'[^\d]+').firstMatch('a1b')![0]!);
  collector.recordString(e: _rx(r'[\d\w]+').firstMatch(' 12ab!')![0]!);

  // The empty class never matches; `[^]` matches anything.
  collector.recordBool(e: _rx('[]').hasMatch('a'));
  collector.recordBool(e: _rx('[]').hasMatch(''));
  collector.recordBool(e: _rx('[]*').hasMatch(''));
  collector.recordString(e: _rx('[^]+').firstMatch('ab')![0]!);
  collector.recordBool(e: _rx('[^]]').hasMatch(']'));
  collector.recordString(e: _rx('a[^]b').firstMatch('xa!b')![0]!);

  // Annex B: an escape next to a range `-` degrades to literals in
  // non-unicode mode (`[\d-z]` is `\d`, `-`, `z`).
  collector.recordString(e: _rx(r'[\d-z]+').firstMatch('a-z9')![0]!);
  collector.recordString(e: _rx(r'[a-\d]+').firstMatch('z-a9')![0]!);
  collector.recordString(e: _rx(r'[\s-a]+').firstMatch('x a')![0]!);
  collector.recordString(e: _rx(r'[\d-x-z]+').firstMatch('q-x-z9')![0]!);
  collector.recordBool(e: _rx(r'[\d-ab-d]').hasMatch('c'));
  collector.recordBool(e: _rx(r'[\d-9]').hasMatch('-'));
  collector.recordBool(e: _rx(r'[\d-]').hasMatch('-'));
  // In unicode mode the same patterns are syntax errors.
  collector.recordBool(e: _throws(() => _rx(r'[\d-z]', u: true).hasMatch('a')));
  collector.recordBool(e: _throws(() => _rx(r'[a-\d]', u: true).hasMatch('a')));
  collector.recordBool(
    e: _throws(() => _rx(r'[\w-\d]', u: true).hasMatch('a')),
  );
  collector.recordBool(e: _throws(() => _rx(r'[\D-z]', u: true).hasMatch('a')));
  // Plain class escapes still compile in unicode mode.
  collector.recordBool(e: _rx(r'[\d]', u: true).hasMatch('1'));
  collector.recordBool(e: _rx(r'[\s]', u: true).hasMatch(' '));

  // Dash placement rules that hold in every mode.
  collector.recordBool(e: _rx('[-z]').hasMatch('-'));
  collector.recordBool(e: _rx(r'[\-]').hasMatch('-'));
  collector.recordBool(e: _rx('[a-]').hasMatch('-'));
  collector.recordBool(e: _rx(r'[\d-a]').hasMatch('a'));
  collector.recordBool(e: _rx('[a-b-c]').hasMatch('c'));

  // Case folding interacts with class members.
  collector.recordBool(e: _rx('[a-z]', i: true).hasMatch('A'));
  collector.recordBool(e: _rx('[A-Z]', i: true).hasMatch('s'));
  collector.recordBool(e: _rx('[é]', i: true).hasMatch('É'));
  collector.recordBool(e: _rx('[É]', i: true).hasMatch('é'));
  collector.recordBool(e: _rx('[à-ÿ]', i: true).hasMatch('É'));
  collector.recordBool(e: _rx('[À-Þ]', i: true).hasMatch('é'));
  collector.recordBool(e: _rx('[e-é]', i: true).hasMatch('µ'));
  collector.recordBool(e: _rx('[µ-ÿ]', i: true).hasMatch('Μ'));
  collector.recordBool(e: _rx('[a-z]', i: true).hasMatch('ſ'));
  collector.recordBool(e: _rx('[a-z]', i: true, u: true).hasMatch('ſ'));
  collector.recordBool(e: _rx('[a-z]', i: true, u: true).hasMatch('\u212A'));
  collector.recordBool(e: _rx('[a-z]', i: true).hasMatch('\u212A'));
  collector.recordBool(e: _rx('[k-s]', i: true, u: true).hasMatch('\u017F'));
  collector.recordBool(e: _rx('[k-s]', i: true).hasMatch('\u017F'));
  collector.recordBool(e: _rx(r'[^\w]', i: true, u: true).hasMatch('ſ'));
  collector.recordBool(e: _rx(r'[\W]', i: true, u: true).hasMatch('ſ'));
  collector.recordBool(e: _rx(r'[\W]', i: true).hasMatch('ſ'));
  // Range order is still checked.
  collector.recordBool(e: _throws(() => _rx('[z-a]').hasMatch('a')));
}

bool _throws(void Function() f) {
  try {
    f();
    return false;
  } on FormatException {
    return true;
  }
}
