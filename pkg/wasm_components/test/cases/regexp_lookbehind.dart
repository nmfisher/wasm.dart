import 'package:test_runner/test_runner.dart';

/// Records the match or the placeholder used for "no match"; the two values
/// cannot be confused with a real match since no pattern here matches the
/// literal word "null".
String? m(RegExp re, String input) => re.firstMatch(input)?.group(0);

String s(String? value) => value ?? 'nomatch';

void main() {
  defineTests(const [
    _lookbehindBasic,
    _lookbehindNegative,
    _lookbehindVariableWidth,
    _lookbehindGroups,
    _lookbehindUnicode,
    _lookbehindAnchors,
  ]);
}

/// Positive lookbehind: the match is only reported when the body matches
/// immediately before, and the reported span covers the body's match only.
void _lookbehindBasic(BaseResultCollector collector) {
  collector.recordString(e: s(RegExp(r'(?<=a)b').firstMatch('ab')?.group(0)));
  collector.recordString(
    e: s(RegExp(r'(?<=a)b').firstMatch('cb')?.group(0)),
  );
  // `^` and start-of-input do not satisfy a lookbehind...
  collector.recordString(e: s(RegExp(r'(?<=^a)b').firstMatch('ab')?.group(0)));
  // ...and a lookbehind at the very start can never match.
  collector.recordString(e: s(RegExp(r'(?<=a)b').firstMatch('aabb')?.group(0)));
  // Leftmost-match semantics still apply with a lookbehind in the pattern.
  final all = RegExp(r'(?<=a)b').allMatches('ab ab').length;
  collector.recordInt(e: all);
  // Class bodies work, including negated and builtin members.
  collector.recordString(
    e: s(RegExp(r'(?<=[0-9])x').firstMatch('a1x')?.group(0)),
  );
  collector.recordString(
    e: s(RegExp(r'(?<=[^\d])x').firstMatch('1x')?.group(0)),
  );
}

/// Negative lookbehind: succeeds where the body does not match, including at
/// the start of the input, where no body can match.
void _lookbehindNegative(BaseResultCollector collector) {
  collector.recordString(
    e: s(RegExp(r'(?<!a)b').firstMatch('cb')?.group(0)),
  );
  collector.recordString(e: s(RegExp(r'(?<!a)b').firstMatch('ab')?.group(0)));
  // Start of input: the body cannot match, so the negative lookbehind does.
  collector.recordString(e: s(RegExp(r'(?<!a)b').firstMatch('b')?.group(0)));
  // Alternation in the body: neither alternative may precede.
  collector.recordString(
    e: s(RegExp(r'(?<!ab|cd)x').firstMatch('cdx')?.group(0)),
  );
  collector.recordString(
    e: s(RegExp(r'(?<!ab|cd)x').firstMatch('efx')?.group(0)),
  );
}

/// Variable-width bodies: quantifiers and alternations of different widths
/// are matched right-to-left, so `(?<=a+)` takes the longest body ending at
/// the position and `(?<=ab|abc)` reports the first alternative that fits.
void _lookbehindVariableWidth(BaseResultCollector collector) {
  collector.recordString(e: s(RegExp(r'(?<=a+)b').firstMatch('aaab')?.group(0)));
  collector.recordString(e: s(RegExp(r'(?<=a*)b').firstMatch('b')?.group(0)));
  collector.recordString(
    e: s(RegExp(r'(?<=ab|abc)b').firstMatch('abcb')?.group(0)),
  );
  // Lazy and greedy quantifiers agree on *whether* the body can match.
  collector.recordString(
    e: s(RegExp(r'(?<=a+?)b').firstMatch('aab')?.group(0)),
  );
  // A `{n,m}` bounded repetition inside the body.
  collector.recordString(
    e: s(RegExp(r'(?<=a{2,3})b').firstMatch('aab')?.group(0)),
  );
  collector.recordString(
    e: s(RegExp(r'(?<=a{3})b').firstMatch('aab')?.group(0)),
  );
}

/// Capturing groups inside lookbehinds: spans are reported like the VM's
/// (the VM gives lookbehind groups their spans), and captures inside a
/// lookbehind that failed to match leave no span behind.
void _lookbehindGroups(BaseResultCollector collector) {
  final m = RegExp(r'(?<=(a))b').firstMatch('ab');
  collector.recordString(e: s(m?.group(0)));
  collector.recordString(e: s(m?.group(1)));
  final m2 = RegExp(r'(?<=(a+))b').firstMatch('aaab');
  collector.recordString(e: s(m2?.group(1)));
  // The group in a *failed* negative-lookbehind body stays unset.
  final m3 = RegExp(r'(?<!x)(b)').firstMatch('cb');
  collector.recordString(e: s(m3?.group(1)));
  // Named groups inside lookbehinds resolve like any other.
  final m4 = RegExp(r'(?<=(?<pre>a))b').firstMatch('ab');
  collector.recordString(e: s(m4?.namedGroup('pre')));
}

/// Unicode mode: astral characters match as one atom in both directions.
void _lookbehindUnicode(BaseResultCollector collector) {
  // U+1F600 occupies a surrogate pair; the lookbehind must step over both.
  collector.recordBool(
    e: RegExp(r'(?<=\u{1F600})b', unicode: true).firstMatch(
          '\u{1F600}b',
        ) !=
        null,
  );
  collector.recordBool(
    e: RegExp(r'(?<=.)b', unicode: true).firstMatch('\u{1F600}b') != null,
  );
  // A class of astral characters, matched right-to-left.
  collector.recordBool(
    e: RegExp(r'(?<=[\u{1F600}\u{1F601}])x', unicode: true).firstMatch(
          '\u{1F601}x',
        ) !=
        null,
  );
  // Without the `u` flag a lone surrogate escape matches only unpaired units.
  collector.recordBool(
    e: RegExp(r'(?<=\uDC00)x').firstMatch('\uDC00x') != null,
  );
}

/// `^`/`$`/`\b` are zero-width, so inside a lookbehind they still test the
/// position they end up at after the reverse walk.
void _lookbehindAnchors(BaseResultCollector collector) {
  // The `^` sits at input start after stepping back over `a`.
  collector.recordBool(
    e: RegExp(r'(?<=a\b)b').firstMatch('ab') != null,
  );
  // `\B` between the two a's.
  collector.recordBool(
    e: RegExp(r'(?<=a\Ba)b').firstMatch('aab') != null,
  );
}
