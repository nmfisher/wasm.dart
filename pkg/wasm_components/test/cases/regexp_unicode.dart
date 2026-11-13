import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_repro]);
}

RegExp _rx(String source, {bool i = false, bool u = false, bool s = false}) =>
    RegExp(source, caseSensitive: !i, unicode: u, dotAll: s);

/// Finding #15: unicode-mode code-point matching and full case folding.
void _repro(BaseResultCollector collector) {
  const scriptX = '\u{1D4B3}'; // 𝒳
  const deseretSmall = '\u{10428}';
  const deseretCapital = '\u{10400}';

  // `.` consumes a whole code point in unicode mode.
  collector.recordBool(e: _rx(r'^.$', u: true).hasMatch(scriptX));
  collector.recordInt(e: _rx(r'^.$', u: true).firstMatch(scriptX)![0]!.length);
  collector.recordBool(e: _rx('a\$b', u: true).hasMatch('a\$b'));
  collector.recordInt(
    e: _rx('^(.*)\$', u: true).firstMatch('a$scriptX')![1]!.length,
  );
  collector.recordBool(e: _rx(r'^.$', u: true).hasMatch('\uD800'));
  collector.recordBool(e: _rx(r'^.$', u: true).hasMatch('\uD800x'));
  collector.recordInt(
    e: _rx('..', u: true).firstMatch('${scriptX}q')![0]!.length,
  );

  // Astral literals and quantifiers.
  collector.recordBool(
    e: _rx('$scriptX{2}', u: true).hasMatch('x${scriptX}x${scriptX}y'),
  );
  collector.recordInt(
    e: _rx('$scriptX+', u: true)
        .firstMatch(
          'q$scriptX$scriptX'
          'q',
        )![0]!
        .length,
  );
  collector.recordBool(e: _rx('$scriptX{2}').hasMatch('$scriptX$scriptX'));
  collector.recordBool(
    e: _rx(r'\u{1D4B3}{2}', u: true).hasMatch('x${scriptX}x${scriptX}y'),
  );
  collector.recordString(
    e: _rx('${scriptX}z', u: true).firstMatch('q${scriptX}z')![0]!,
  );
  collector.recordInt(
    e: _rx(
      '$scriptX{2,3}',
      u: true,
    ).firstMatch('$scriptX$scriptX$scriptX')![0]!.length,
  );
  collector.recordInt(
    e: _rx('($scriptX+)', u: true).firstMatch('$scriptX$scriptX')![1]!.length,
  );

  // Astral character classes.
  collector.recordString(
    e: _rx(
      '[$scriptX z]+'.replaceAll(' ', ''),
      u: true,
    ).firstMatch('q${scriptX}z')![0]!,
  );
  collector.recordBool(e: _rx('[$scriptX]', u: true).hasMatch(scriptX));
  collector.recordString(e: _rx('[^z]', u: true).firstMatch(scriptX)![0]!);
  collector.recordString(
    e: _rx(r'[^\d]', u: true).firstMatch('1$scriptX')![0]!,
  );
  collector.recordBool(
    e: _rx('[\u{1D4B0}-\u{1D4BF}]', u: true).hasMatch(scriptX),
  );
  collector.recordBool(
    e: _rx('[^\u{1D4B0}-\u{1D4BF}]', u: true).hasMatch(scriptX),
  );
  collector.recordBool(
    e: _rx('[\u{1D4B0}-\u{1D4BF}]', u: true).hasMatch('\u{1D400}'),
  );
  collector.recordString(e: _rx('[^]', u: true).firstMatch('ab')![0]!);

  // Lone surrogates match only unpaired ones in unicode mode.
  collector.recordBool(e: _rx('\uD800', u: true).hasMatch('\uD800'));
  collector.recordBool(e: _rx('\uD800', u: true).hasMatch('\uD800\uDC00'));
  collector.recordBool(e: _rx('\\uDC00', u: true).hasMatch('a\uDC00'));
  collector.recordBool(e: _rx('\\uDC00', u: true).hasMatch('\uD800\uDC00'));
  collector.recordBool(
    e: _rx('\\uD800\\uDC00', u: true).hasMatch('\uD800\uDC00'),
  );
  collector.recordInt(
    e: _rx('\\uD800\\uDC00', u: true).firstMatch('\uD800\uDC00')![0]!.length,
  );
  collector.recordBool(
    e: _rx('\uD800\uDC00', u: true).hasMatch('\uD800\uDC00'),
  );
  collector.recordBool(e: _rx('\uD800').hasMatch('\uD800'));

  // Builtins consume code points in unicode mode.
  collector.recordBool(e: _rx(r'^\D$', u: true).hasMatch(scriptX));
  collector.recordBool(e: _rx(r'^\D$').hasMatch(scriptX));
  collector.recordInt(
    e: _rx(r'\D', u: true).firstMatch('1$scriptX')![0]!.length,
  );
  collector.recordBool(e: _rx(r'^\w$', u: true).hasMatch(scriptX));

  // Case folding beyond ASCII.
  collector.recordBool(e: _rx('é', i: true).hasMatch('É'));
  collector.recordBool(e: _rx('É', i: true).hasMatch('é'));
  collector.recordBool(e: _rx('[é]', i: true).hasMatch('É'));
  collector.recordBool(e: _rx('σ', i: true).hasMatch('ς'));
  collector.recordBool(e: _rx('Σ', i: true).hasMatch('σ'));
  collector.recordBool(e: _rx('µ', i: true).hasMatch('μ'));
  collector.recordBool(e: _rx('µ', i: true, u: true).hasMatch('Μ'));
  collector.recordBool(e: _rx('k', i: true, u: true).hasMatch('\u212A'));
  collector.recordBool(e: _rx('k', i: true).hasMatch('\u212A'));
  collector.recordBool(e: _rx('s', i: true, u: true).hasMatch('\u017F'));
  collector.recordBool(e: _rx('s', i: true).hasMatch('\u017F'));
  collector.recordBool(e: _rx('ß', i: true, u: true).hasMatch('ẞ'));
  collector.recordBool(e: _rx('ß', i: true).hasMatch('ẞ'));
  collector.recordBool(e: _rx('Å', i: true, u: true).hasMatch('\u212B'));
  collector.recordBool(e: _rx('Å', i: true).hasMatch('\u212B'));
  collector.recordBool(e: _rx('Ω', i: true, u: true).hasMatch('\u2126'));
  collector.recordBool(e: _rx('İ', i: true).hasMatch('i'));
  collector.recordBool(e: _rx('ı', i: true).hasMatch('I'));
  collector.recordBool(e: _rx(r'\w', i: true, u: true).hasMatch('\u017F'));
  collector.recordBool(e: _rx(r'\w', i: true).hasMatch('\u017F'));
  collector.recordBool(
    e: _rx('[$deseretSmall]', i: true, u: true).hasMatch(deseretCapital),
  );
  collector.recordBool(
    e: _rx(deseretSmall, i: true, u: true).hasMatch(deseretCapital),
  );
  collector.recordBool(e: _rx(deseretSmall, i: true).hasMatch(deseretCapital));

  // Backreferences fold like literals; astral groups fold as code points.
  collector.recordBool(e: _rx(r'(σ)\1', i: true).hasMatch('σς'));
  collector.recordBool(e: _rx(r'(σ)\1', i: true, u: true).hasMatch('σς'));
  collector.recordBool(e: _rx(r'(σ)\1', u: true).hasMatch('σς'));
  collector.recordBool(
    e: _rx(
      '($deseretSmall)\\1',
      i: true,
      u: true,
    ).hasMatch('$deseretSmall$deseretCapital'),
  );
  collector.recordBool(e: _rx(r'(k)\1', i: true, u: true).hasMatch('k\u212A'));

  // The search never starts inside a surrogate pair.
  collector.recordInt(
    e: _rx(
      '',
      u: true,
    ).allMatches('\uD800\uDC00').map((m) => m.start).toList().length,
  );
  collector.recordString(
    e: _rx(
      '.*',
      u: true,
    ).allMatches('\uD800\uDC00').map((m) => '${m.start}-${m.end}').join(','),
  );
  collector.recordString(
    e: _rx(
      '[^z]',
      u: true,
    ).allMatches('\uD800\uDC00x').map((m) => m[0]!).join(','),
  );
  collector.recordInt(
    e: _rx('x', u: true).firstMatch('\uD800\uDC00x')?.start ?? -1,
  );
  collector.recordString(
    e: _rx('.*', u: true).matchAsPrefix('\uD800\uDC00', 1)?[0] ?? 'null',
  );
  collector.recordString(
    e: _rx('.', u: true).matchAsPrefix('\uD800\uDC00', 1)?[0] ?? 'null',
  );
  collector.recordBool(
    e: _rx(r'\babc\b', u: true).hasMatch('\u{10000}abc\u{10000}'),
  );
}
