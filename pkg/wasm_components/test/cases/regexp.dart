import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [
    _basics,
    _compileErrors,
    _escape,
    _anchors,
    _charClasses,
    _quantifiers,
    _alternation,
    _groups,
    _namedGroups,
    _backreferences,
    _lookarounds,
    _flags,
    _matchAsPrefix,
    _replaceAllRegExp,
    _stringMatchIterator,
  ]);
}

RegExp _rx(String source, {bool m = false, bool i = false, bool s = false, bool u = false}) =>
    RegExp(source, multiLine: m, caseSensitive: !i, unicode: u, dotAll: s);

void _basics(BaseResultCollector collector) {
  collector.recordBool(e: RegExp('a').hasMatch('abc'));
  collector.recordBool(e: RegExp('z').hasMatch('abc'));
  collector.recordString(e: RegExp('a+b').firstMatch('caabd')![0]!);
  collector.recordInt(e: RegExp('a+b').firstMatch('caabd')!.start);
  collector.recordInt(e: RegExp('a+b').firstMatch('caabd')!.end);
  collector.recordInt(e: RegExp('a+b').firstMatch('caabd')!.groupCount);
}

void _compileErrors(BaseResultCollector collector) {
  // Compile errors surface as FormatException('Illegal RegExp pattern (...)').
  try {
    // ignore: valid_regexps
    RegExp('(');
    collector.recordBool(e: false);
  } on FormatException {
    collector.recordBool(e: true);
  }
  try {
    // ignore: valid_regexps
    RegExp('a\\');
    collector.recordBool(e: false);
  } on FormatException {
    collector.recordBool(e: true);
  }
  try {
    // ignore: valid_regexps
    RegExp('[z-a]');
    collector.recordBool(e: false);
  } on FormatException {
    collector.recordBool(e: true);
  }
}

void _escape(BaseResultCollector collector) {
  collector.recordString(e: RegExp.escape('Hello world'));
  collector.recordString(e: RegExp.escape(r'1+1=2'));
  collector.recordString(e: RegExp.escape(r'a$b(c)d*e.f?g[h]i\j^k{l|m}'));
  collector.recordBool(e: RegExp(RegExp.escape(r'1+1')).hasMatch('1+1=2'));
  collector.recordBool(e: RegExp(RegExp.escape(r'1+1')).hasMatch('111'));
}

void _anchors(BaseResultCollector collector) {
  collector.recordBool(e: RegExp(r'^abc$').hasMatch('abc'));
  collector.recordBool(e: RegExp(r'^abc$').hasMatch('xabc'));
  collector.recordBool(e: RegExp(r'^abc$').hasMatch('abcx'));
  // Non-multiline `^` never matches past index 0.
  collector.recordBool(e: RegExp(r'^b').hasMatch('ab'));
  collector.recordBool(e: _rx('^b', m: true).hasMatch('ab'));
  collector.recordString(e: _rx(r'^\w+', m: true).firstMatch('ab\ncd')![0]!);
  collector.recordString(e: _rx(r'\w+$', m: true).firstMatch('ab\ncd')![0]!);
  collector.recordBool(e: RegExp(r'\bword\b').hasMatch('a word here'));
  collector.recordBool(e: RegExp(r'\bword\b').hasMatch('sword'));
  collector.recordBool(e: RegExp(r'\Bo').hasMatch('word'));
}

void _charClasses(BaseResultCollector collector) {
  collector.recordString(e: RegExp(r'\d+').firstMatch('ab123cd')![0]!);
  collector.recordString(e: RegExp(r'[^0-9]+').firstMatch('ab123cd')![0]!);
  collector.recordBool(e: RegExp(r'\D').hasMatch('123a'));
  collector.recordBool(e: RegExp(r'\D').hasMatch('123'));
  collector.recordString(e: RegExp('[a-c]{3}').firstMatch('zzabczz')![0]!);
  collector.recordBool(e: RegExp('[abc]+').hasMatch('xyz'));
  // Class-internal escapes that we support.
  collector.recordString(e: RegExp(r'[\t\n]').firstMatch('a\nb')![0]!);
  collector.recordString(e: RegExp(r'[\x41]').firstMatch('xAy')![0]!);
}

void _quantifiers(BaseResultCollector collector) {
  collector.recordString(e: RegExp('a{2,3}').firstMatch('aaaa')![0]!);
  collector.recordString(e: RegExp('a{2,3}?').firstMatch('aaaa')![0]!);
  collector.recordString(e: RegExp('a*').firstMatch('bbb')![0]!);
  collector.recordString(e: RegExp('<.*>').firstMatch('<a><b>')![0]!);
  collector.recordString(e: RegExp('<.*?>').firstMatch('<a><b>')![0]!);
  collector.recordBool(e: RegExp('a?').hasMatch(''));
}

void _alternation(BaseResultCollector collector) {
  collector.recordString(e: RegExp('cat|dog').firstMatch('hotdog')![0]!);
  collector.recordString(e: RegExp('foo|foobar').firstMatch('foobar')![0]!);
  collector.recordBool(e: RegExp(r'^(?:a|b)+$').hasMatch('abab'));
  collector.recordBool(e: RegExp(r'^(?:a|b)+$').hasMatch('abc'));
}

void _groups(BaseResultCollector collector) {
  final match = RegExp(r'(\w+)-(\w+)').firstMatch('big-red')!;
  collector.recordString(e: match[0]!);
  collector.recordString(e: match[1]!);
  collector.recordString(e: match[2]!);
  collector.recordInt(e: match.groupCount);
  // Nested groups.
  final nested = RegExp(r'((a)(b))?').firstMatch('ab')!;
  collector.recordInt(e: nested.groupCount);
  collector.recordString(e: nested[1]!);
  collector.recordString(e: nested[3]!);
  // Non-participating group.
  final either = RegExp(r'(x)|(y)').firstMatch('y')!;
  collector.recordString(e: either[1] ?? 'unset');
  collector.recordString(e: either[2]!);
}

void _namedGroups(BaseResultCollector collector) {
  final match = RegExp(r'(?<year>\d{4})-(?<month>\d{2})').firstMatch('2026-09')!;
  collector.recordString(e: match.namedGroup('year')!);
  collector.recordString(e: match.namedGroup('month')!);
  collector.recordInt(e: match.groupCount);
  collector.recordString(e: match.groupNames.join(','));
  try {
    match.namedGroup('nope');
    collector.recordBool(e: false);
  } on ArgumentError {
    collector.recordBool(e: true);
  }
}

void _backreferences(BaseResultCollector collector) {
  collector.recordBool(e: RegExp(r'(\w) \1').hasMatch('hey hey'));
  collector.recordBool(e: RegExp(r'(\w) \1').hasMatch('hey hoo'));
  collector.recordString(e: RegExp(r'(\w)\1+').firstMatch('aaabbb')![0]!);
}

void _lookarounds(BaseResultCollector collector) {
  collector.recordString(e: RegExp(r'foo(?=bar)').firstMatch('foobar')![0]!);
  collector.recordBool(e: RegExp(r'foo(?=bar)').hasMatch('foobaz'));
  collector.recordString(e: RegExp(r'foo(?!bar)').firstMatch('foobaz')![0]!);
  collector.recordBool(e: RegExp(r'foo(?!bar)').hasMatch('foobar'));
}

void _flags(BaseResultCollector collector) {
  collector.recordBool(e: _rx('HELLO', i: true).hasMatch('hello'));
  collector.recordBool(e: _rx('HELLO', i: true).hasMatch('HELLO'));
  collector.recordBool(e: _rx('HELLO').hasMatch('hello'));
  collector.recordBool(e: _rx('a.c', s: true).hasMatch('a\nc'));
  collector.recordBool(e: _rx('a.c').hasMatch('a\nc'));
  collector.recordBool(e: _rx('a.c').hasMatch('axc'));
  // Unicode mode handles surrogate pairs as code points.
  collector.recordBool(e: _rx('𝄞', u: true).hasMatch('a𝄞b'));
  collector.recordString(e: _rx(r'\u{1D11E}', u: true).firstMatch('a𝄞b')![0]!);
}

void _matchAsPrefix(BaseResultCollector collector) {
  collector.recordBool(e: RegExp('bc').matchAsPrefix('abc', 1) != null);
  collector.recordBool(e: RegExp('bc').matchAsPrefix('abc', 0) != null);
  collector.recordBool(e: RegExp('abc').matchAsPrefix('abc', 0) != null);
  collector.recordBool(e: RegExp('abc').matchAsPrefix('xabc', 1) == null);
  final match = RegExp(r'\w+').matchAsPrefix('hello world', 6)!;
  collector.recordString(e: match[0]!);
  collector.recordInt(e: match.start);
}

void _replaceAllRegExp(BaseResultCollector collector) {
  collector.recordString(e: 'one two three'.replaceAll(RegExp('two'), '2'));
  collector.recordString(e: 'aaa'.replaceAll(RegExp('a'), '-'));
  collector.recordString(e: 'abcabc'.replaceAll(RegExp('b'), ''));
  collector.recordString(e: 'a-b-c'.replaceAll(RegExp(r'\d'), 'N'));
  collector.recordString(e: 'abc'.replaceAll(RegExp('x'), 'N'));
  collector.recordString(e: 'caab'.replaceAll(RegExp('a+'), 'X'));
  // Zero-width matches must not loop forever.
  collector.recordString(e: 'ab'.replaceAll(RegExp(r'\b'), '|'));
  collector.recordString(e: ''.replaceAll(RegExp('a'), 'x'));
}

void _stringMatchIterator(BaseResultCollector collector) {
  collector.recordInt(e: RegExp('a').allMatches('banana').length);
  collector.recordString(e: RegExp('a').allMatches('banana').last[0]!);
  collector.recordInt(e: RegExp('a').allMatches('banana').last.start);
  collector.recordInt(e: RegExp('z').allMatches('banana').length);
}
