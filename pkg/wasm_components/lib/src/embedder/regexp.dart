/// A regular-expression engine implementing the `dart.regexp*` embedder
/// imports on top of the custom [WasmStringImplementation] strings.
///
/// The standalone wasm target implements `RegExp` with JavaScript semantics
/// (ECMAScript `RegExp` over UTF-16 code units): the leftmost match wins, and
/// among matches at the same start the pattern's preference order (alternatives
/// first-to-last, greedy by default, lazy after `?`) decides which match is
/// reported.
///
/// Supported syntax: literals, `.`, escapes (`\d \D \w \W \s \S \b \B \n \r
/// \t \f \v \0 \cX \xhh \uhhhh \u{...}`, identity escapes, backreferences),
/// character classes with ranges and negation, quantifiers (`*`, `+`, `?`,
/// `{n}`, `{n,}`, `{n,m}`, greedy and lazy), groups (`(...)`, `(?:...)`,
/// `(?<name>...)`), alternation, lookaheads `(?=...)` / `(?!...)`, anchors
/// `^`/`$`, and the flags `i m s u`.
library;

// ignore: import_internal_library
import 'dart:_wasm';

import 'string.dart';

/// A compiled regular expression.
final class EmbedderRegexp {
  final String patternSource;
  final bool isMultiLine;
  final bool isCaseSensitive;
  final bool isUnicode;
  final bool isDotAll;
  final bool isSticky;
  final bool isGlobal;

  /// The parsed top-level alternatives. Each alternative is a list of nodes
  /// that must match in order.
  // Library-private so the internal node type does not leak into the API.
  final List<List<_RegexpNode>> _alternatives;

  /// Number of capturing groups including group 0.
  final int groupCount;

  /// Name of group [i], or null. Index 0 (the whole match) is unnamed.
  final List<String?> groupNames;

  /// Index of the group called [name].
  final Map<String, int> groupIndicesByName;

  EmbedderRegexp._(
    this.patternSource,
    this.isMultiLine,
    this.isCaseSensitive,
    this.isUnicode,
    this.isDotAll,
    this.isSticky,
    this.isGlobal,
    this._alternatives,
    this.groupCount,
    this.groupNames,
    this.groupIndicesByName,
  );

  /// Compiles [source], or returns the error message as a [String]
  /// (`dart.regexpCreateOrFailWithString` contract).
  static Object compile(
    String source,
    bool multiLine,
    bool caseSensitive,
    bool unicode,
    bool dotAll,
  ) {
    try {
      final parser = _RegexpParser(
        source: source,
        unicode: unicode,
        caseInsensitive: !caseSensitive,
        dotAll: dotAll,
        multiLine: multiLine,
      );
      final alts = parser.parseAlternatives();
      return EmbedderRegexp._(
        source,
        multiLine,
        caseSensitive,
        unicode,
        dotAll,
        parser.isSticky,
        parser.isGlobal,
        alts,
        parser.groupCount,
        parser.groupNames,
        parser.groupIndicesByName,
      );
    } on FormatException catch (exception) {
      // The error message crosses the wasm boundary as a string object; it
      // has to be one of our string implementations, not a plain Dart string.
      return WasmStringImplementation.fromDartString(exception.message);
    }
  }

  /// The first match at or after [start], or exactly at [start] when
  /// [asPrefix] is set (`RegExp.matchAsPrefix`).
  EmbedderRegexpMatch? match(
    WasmStringImplementation string,
    int start,
    bool asPrefix,
  ) {
    final length = string.length;
    if (start < 0 || start > length) return null;
    final limit = asPrefix ? start : length;
    for (var position = start; position <= limit; position++) {
      final captures = WasmArray<WasmI32>(groupCount > 0 ? groupCount - 1 : 0);
      final matcher = _RegexpMatcher(this, string, captures);
      final end = matcher.search(position, asPrefix);
      if (end != null) {
        return EmbedderRegexpMatch(this, string, position, end, captures);
      }
      if (asPrefix) return null;
    }
    return null;
  }

  /// Replaces every match with [replacement] (`dart.stringReplaceAllRegExp`;
  /// the SDK passes the already-compiled regexp object as the pattern).
  ///
  /// The SDK's `String.replaceAll` does not consult `$` group references in
  /// [replacement] for the embedder path, so the replacement is inserted
  /// verbatim, like the string-based specialization.
  WasmStringImplementation replaceAllRegExp(
    WasmStringImplementation string,
    WasmStringImplementation replacement,
  ) {
    final length = string.length;

    // Pass 1: find the matches and size the result exactly.
    final spans = <List<int>>[]; // [matchStart, matchEnd, copyFrom] triples.
    var resultLength = 0;
    var searchFrom = 0;
    while (searchFrom <= length) {
      final found = match(string, searchFrom, false);
      if (found == null) break;
      if (found.start > searchFrom) {
        spans.add([searchFrom, found.start, 1]);
        resultLength += found.start - searchFrom;
      }
      spans.add([0, 0, 0]); // 0/0/0 marks the replacement.
      resultLength += replacement.length;
      if (found.end == found.start) {
        // Zero-width match: copy the code unit the match consumed nothing of
        // and step over it, so the loop terminates.
        if (found.end < length) {
          spans.add([found.end, found.end + 1, 1]);
          resultLength++;
        }
        searchFrom = found.end + 1;
      } else {
        searchFrom = found.end;
      }
    }
    if (searchFrom < length) {
      spans.add([searchFrom, length, 1]);
      resultLength += length - searchFrom;
    }

    // Pass 2: write the result in one allocation.
    final result = WasmArray<WasmI16>(resultLength);
    var offset = 0;
    for (final span in spans) {
      if (span[2] == 0) {
        for (var i = 0; i < replacement.length; i++) {
          result.write(offset++, replacement.codeUnitAtUnchecked(i));
        }
      } else {
        for (var i = span[0]; i < span[1]; i++) {
          result.write(offset++, string.codeUnitAtUnchecked(i));
        }
      }
    }
    return Utf16String.unsafeWrap(result);
  }

  /// Escapes [text] for verbatim use inside a pattern. JavaScript escapes
  /// exactly `$ ( ) * + . ? [ \ ] ^ { | }` (`dart.regexpEscape`).
  static WasmStringImplementation escape(WasmStringImplementation text) {
    final length = text.length;
    var escapes = 0;
    for (var i = 0; i < length; i++) {
      if (_needsEscape(text.codeUnitAtUnchecked(i))) escapes++;
    }
    if (escapes == 0) return text;
    if (length == 0) return text;
    if (text is Latin1String) {
      final out = WasmArray<WasmI8>(length + escapes);
      var offset = 0;
      for (var i = 0; i < length; i++) {
        final code = text.codeUnitAtUnchecked(i);
        if (_needsEscape(code)) out.write(offset++, 0x5c);
        out.write(offset++, code);
      }
      return Latin1String.unsafeWrap(out);
    }
    final out = WasmArray<WasmI16>(length + escapes);
    var offset = 0;
    for (var i = 0; i < length; i++) {
      final code = text.codeUnitAtUnchecked(i);
      if (_needsEscape(code)) out.write(offset++, 0x5c);
      out.write(offset++, code);
    }
    return Utf16String.unsafeWrap(out);
  }

  static bool _needsEscape(int code) {
    switch (code) {
      case 0x24 || 0x28 || 0x29 || 0x2a || 0x2b || 0x2e || 0x3f:
      case 0x5b || 0x5c || 0x5d || 0x5e || 0x7b || 0x7c || 0x7d:
        return true;
      default:
        return false;
    }
  }
}

/// A match of an [EmbedderRegexp] against an input string.
final class EmbedderRegexpMatch {
  final EmbedderRegexp pattern;
  final WasmStringImplementation input;

  /// Code-unit offset where the match starts.
  final int start;

  /// Code-unit offset just past the match.
  final int end;

  /// Capture spans; element [i] holds group `i + 1` packed as
  /// `start | (end << 20)`, or 0 when the group did not participate.
  final WasmArray<WasmI32> captures;
  EmbedderRegexpMatch(
    this.pattern,
    this.input,
    this.start,
    this.end,
    this.captures,
  );

  /// The [index]th group (`dart.regexpMatchGetGroup` contract: the SDK only
  /// asks for indices between 0 and [groupCount] inclusive).
  WasmStringImplementation? group(int index) {
    if (index == 0) {
      return input.substring(WasmI32.fromInt(start), WasmI32.fromInt(end));
    }
    final packed = captures.readUnsigned(index - 1) & 0xFFFFFFFF;
    if (packed == 0) return null;
    final begin = packed & 0xFFFFF;
    final finish = (packed >>> 20) & 0xFFFFF;
    return input.substring(WasmI32.fromInt(begin), WasmI32.fromInt(finish));
  }

  /// Number of capturing groups excluding group 0 (the whole match): the
  /// `RegExpMatch.groupCount` contract.
  int get groupCount => pattern.groupCount - 1;
}

// ---------------------------------------------------------------------------
// Compiled pattern representation
// ---------------------------------------------------------------------------

/// A node in the compiled pattern: [matchAt] tries to match the node at
/// [position], continuing through [next] on success.
abstract class _RegexpNode {
  /// Whether this node can match without consuming input.
  bool get canMatchEmpty;

  /// Tries to match at [position]; on success continues with [next] and
  /// returns the end of the whole match, otherwise returns null.
  int? matchAt(
    int position,
    _RegexpMatcher matcher,
    _MatchContinuation next,
  );
}

/// The rest of the match, chained so backtracking works by construction.
abstract class _MatchContinuation {
  const _MatchContinuation();

  int? run(int position);
}

/// The continuation parameter type visible in the public [_RegexpNode.matchAt]
/// signature.

class _Halt extends _MatchContinuation {
  const _Halt();

  @override
  int? run(int position) => position;
}

/// Continues with [nodes[index]], then [next].
class _Sequence extends _MatchContinuation {
  final List<_RegexpNode> nodes;
  final int index;
  final _RegexpMatcher matcher;
  final _MatchContinuation next;

  _Sequence(this.nodes, this.index, this.matcher, this.next);

  @override
  int? run(int position) {
    if (index >= nodes.length) return next.run(position);
    // Continue with the rest of this sequence before handing off to [next];
    // the sequence has to walk its own nodes in order.
    return nodes[index].matchAt(
      position,
      matcher,
      _Sequence(nodes, index + 1, matcher, next),
    );
  }
}

// ---------------------------------------------------------------------------
// Matcher
// ---------------------------------------------------------------------------

/// The state of one match attempt.
class _RegexpMatcher {
  final EmbedderRegexp pattern;
  final WasmStringImplementation string;
  final WasmArray<WasmI32> captures;
  final int inputLength;

  /// Where the current trial was anchored. In prefix mode `^` only matches
  /// here (or after a line terminator in multiline mode); in search mode `^`
  /// anchors to index 0 like JavaScript's non-multiline `^`.
  int trialStart = 0;

  _RegexpMatcher(this.pattern, this.string, this.captures)
      : inputLength = string.length;

  /// Matches the whole pattern starting exactly at [position] when [asPrefix]
  /// is set, otherwise finds the leftmost match at or after [position].
  /// Returns the end of the match or null.
  int? search(int position, bool asPrefix) {
    trialStart = asPrefix ? position : 0;
    return _tryAlternatives(
      pattern._alternatives,
      this,
      const _Halt(),
      position,
    );
  }

  bool codeUnitEqualsAt(int position, int code) {
    if (position >= inputLength) return false;
    final actual = string.codeUnitAtUnchecked(position);
    if (actual == code) return true;
    if (pattern.isCaseSensitive) return false;
    return _foldEquals(actual, code);
  }

  bool isWordCharAt(int position) {
    if (position < 0 || position >= inputLength) return false;
    final code = string.codeUnitAtUnchecked(position);
    return code == 0x5f ||
        (code >= 0x30 && code <= 0x39) ||
        (code >= 0x41 && code <= 0x5a) ||
        (code >= 0x61 && code <= 0x7a);
  }

  bool isLineTerminatorAt(int position) {
    if (position < 0 || position >= inputLength) return false;
    final code = string.codeUnitAtUnchecked(position);
    return code == 0x0a || code == 0x0d || code == 0x2028 || code == 0x2029;
  }
}

bool _foldEquals(int actual, int expected) {
  if (actual >= 0x41 && actual <= 0x5a) actual += 0x20;
  if (expected >= 0x41 && expected <= 0x5a) expected += 0x20;
  return actual == expected;
}

// ---------------------------------------------------------------------------
// Concrete nodes
// ---------------------------------------------------------------------------

/// A single literal code unit, case-folded on demand.
final class _CharNode extends _RegexpNode {
  final int code;
  final bool caseInsensitive;

  _CharNode(this.code, this.caseInsensitive);

  @override
  bool get canMatchEmpty => false;

  @override
  int? matchAt(int position, _RegexpMatcher matcher, _MatchContinuation next) {
    if (!matcher.codeUnitEqualsAt(position, code)) return null;
    return next.run(position + 1);
  }
}

/// A code point above the BMP (`\u{...}` escape or a literal astral
/// character in unicode mode). Stored as one code point and matched against
/// the subject's surrogate pair; case-insensitive folding is ASCII-only, so
/// it does not apply here.
final class _AstralCharNode extends _RegexpNode {
  final int code;

  _AstralCharNode(this.code);

  @override
  bool get canMatchEmpty => false;

  @override
  int? matchAt(int position, _RegexpMatcher matcher, _MatchContinuation next) {
    final offset = code - 0x10000;
    final high = 0xD800 + (offset >> 10);
    final low = 0xDC00 + (offset & 0x3FF);
    if (!matcher.codeUnitEqualsAt(position, high)) return null;
    if (!matcher.codeUnitEqualsAt(position + 1, low)) return null;
    return next.run(position + 2);
  }
}

/// Builds the node for a parsed code point: astral code points in unicode
/// mode match the subject's surrogate pair as one atom.
_RegexpNode _charNodeForCodePoint(int code, bool caseInsensitive, bool unicode) {
  if (unicode && code > 0xFFFF) return _AstralCharNode(code);
  return _CharNode(code, caseInsensitive);
}

/// Any code unit except line terminators, unless `s` is set.
final class _DotNode extends _RegexpNode {
  final bool dotAll;

  _DotNode(this.dotAll);

  @override
  bool get canMatchEmpty => false;

  @override
  int? matchAt(int position, _RegexpMatcher matcher, _MatchContinuation next) {
    if (position >= matcher.inputLength) return null;
    if (!dotAll && matcher.isLineTerminatorAt(position)) return null;
    return next.run(position + 1);
  }
}

/// `^` or `$`.
final class _AnchorNode extends _RegexpNode {
  final bool isDollar;
  final bool isMultiLine;

  _AnchorNode(this.isDollar, this.isMultiLine);

  @override
  bool get canMatchEmpty => true;

  @override
  int? matchAt(int position, _RegexpMatcher matcher, _MatchContinuation next) {
    if (isDollar) {
      final ok = isMultiLine
          ? position == matcher.inputLength ||
              matcher.isLineTerminatorAt(position)
          : position == matcher.inputLength;
      return ok ? next.run(position) : null;
    }
    final ok = isMultiLine
        ? position == matcher.trialStart ||
            matcher.isLineTerminatorAt(position - 1)
        : position == matcher.trialStart;
    return ok ? next.run(position) : null;
  }
}

/// `\b` or `\B`.
final class _WordBoundaryNode extends _RegexpNode {
  final bool isNegated;

  _WordBoundaryNode(this.isNegated);

  @override
  bool get canMatchEmpty => true;

  @override
  int? matchAt(int position, _RegexpMatcher matcher, _MatchContinuation next) {
    final before = matcher.isWordCharAt(position - 1);
    final after = matcher.isWordCharAt(position);
    final atBoundary = before != after;
    return atBoundary != isNegated ? next.run(position) : null;
  }
}

/// `\d`, `\D`, `\w`, `\W`, `\s` or `\S`.
final class _BuiltinClassNode extends _RegexpNode {
  /// The ASCII code of the escape letter: `d D w W s S`.
  final int kind;

  _BuiltinClassNode(this.kind);

  @override
  bool get canMatchEmpty => false;

  @override
  int? matchAt(int position, _RegexpMatcher matcher, _MatchContinuation next) {
    if (position >= matcher.inputLength) return null;
    final code = matcher.string.codeUnitAtUnchecked(position);
    var inside = switch (kind) {
      0x64 || 0x44 => code >= 0x30 && code <= 0x39,
      0x77 || 0x57 => code == 0x5f ||
          (code >= 0x30 && code <= 0x39) ||
          (code >= 0x41 && code <= 0x5a) ||
          (code >= 0x61 && code <= 0x7a),
      0x73 || 0x53 => _isWhitespace(code),
      _ => false,
    };
    if (kind == 0x44 || kind == 0x57 || kind == 0x53) inside = !inside;
    return inside ? next.run(position + 1) : null;
  }

  static bool _isWhitespace(int code) {
    switch (code) {
      case 0x09 || 0x0a || 0x0b || 0x0c || 0x0d || 0x20:
      case 0xa0 || 0x1680:
      case >= 0x2000 && <= 0x200a:
      case 0x2028 || 0x2029 || 0x202f || 0x205f || 0x3000 || 0xfeff:
        return true;
      default:
        return false;
    }
  }
}

/// A character class `[...]`: ranges of code units.
final class _ClassNode extends _RegexpNode {
  final List<(int, int)> ranges;
  final bool negated;

  _ClassNode(this.ranges, this.negated);

  @override
  bool get canMatchEmpty => false;

  @override
  int? matchAt(int position, _RegexpMatcher matcher, _MatchContinuation next) {
    if (position >= matcher.inputLength) return null;
    final code = matcher.string.codeUnitAtUnchecked(position);
    var inside = _contains(code);
    if (!inside && !matcher.pattern.isCaseSensitive) {
      // Fold the input and retry: covers `[a-z]` against 'A' style cases.
      final lower = code >= 0x41 && code <= 0x5a ? code + 0x20 : code;
      final upper = code >= 0x61 && code <= 0x7a ? code - 0x20 : code;
      inside = (lower != code && _contains(lower)) ||
          (upper != code && _contains(upper));
    }
    return inside != negated ? next.run(position + 1) : null;
  }

  bool _contains(int code) {
    for (var i = 0; i < ranges.length; i++) {
      final range = ranges[i];
      if (code < range.$1) return false;
      if (code <= range.$2) return true;
    }
    return false;
  }
}

/// A group: `(…)`, `(?:…)`, `(?<name>…)`, `(?=…)` or `(?!…)`.
final class _GroupNode extends _RegexpNode {
  final List<List<_RegexpNode>> alternatives;
  final int captureIndex;

  /// 0 non-capturing, 1 capturing, 2 lookahead `(?=`, 3 negative lookahead
  /// `(?!`.
  final int kind;

  _GroupNode(this.alternatives, this.captureIndex, this.kind);

  @override
  bool get canMatchEmpty {
    for (var a = 0; a < alternatives.length; a++) {
      final nodes = alternatives[a];
      var possible = true;
      for (var i = 0; i < nodes.length; i++) {
        if (!nodes[i].canMatchEmpty) {
          possible = false;
          break;
        }
      }
      if (possible) return true;
    }
    return false;
  }

  @override
  int? matchAt(int position, _RegexpMatcher matcher, _MatchContinuation next) {
    switch (kind) {
      case 2:
        return _tryLookahead(alternatives, matcher, next, position, true);
      case 3:
        return _tryLookahead(alternatives, matcher, next, position, false);
      case 1:
        return _tryCapturing(
          alternatives,
          captureIndex,
          matcher,
          next,
          position,
        );
      default:
        return _tryAlternatives(alternatives, matcher, _GroupEnd(next),
            position);
    }
  }
}

/// Tries each alternative in order; returns the first full continuation
/// success.
int? _tryAlternatives(
  List<List<_RegexpNode>> alternatives,
  _RegexpMatcher matcher,
  _MatchContinuation next,
  int position,
) {
  for (var a = 0; a < alternatives.length; a++) {
    final result = _Sequence(alternatives[a], 0, matcher, next).run(position);
    if (result != null) return result;
  }
  return null;
}

/// Lookahead: on a (non-)match resumes [next] at the lookahead start.
int? _tryLookahead(
  List<List<_RegexpNode>> alternatives,
  _RegexpMatcher matcher,
  _MatchContinuation next,
  int position,
  bool positive,
) {
  final matched =
      _tryAlternatives(alternatives, matcher, const _Halt(), position) != null;
  if (matched == positive) return next.run(position);
  return null;
}

/// Capturing group: tries each alternative; when the body matched, the span
/// is recorded, then [next] runs. If [next] (or a later alternative) fails,
/// the previous span is restored before the next alternative is tried.
int? _tryCapturing(
  List<List<_RegexpNode>> alternatives,
  int captureIndex,
  _RegexpMatcher matcher,
  _MatchContinuation next,
  int position,
) {
  final slot = captureIndex - 1;
  final previous = matcher.captures.readUnsigned(slot) & 0xFFFFFFFF;
  return _tryCapturingFrom(
    alternatives,
    0,
    slot,
    previous,
    matcher,
    next,
    position,
  );
}

int? _tryCapturingFrom(
  List<List<_RegexpNode>> alternatives,
  int alternativeIndex,
  int slot,
  int previous,
  _RegexpMatcher matcher,
  _MatchContinuation next,
  int position,
) {
  if (alternativeIndex >= alternatives.length) {
    // All alternatives failed: put back the span the group had before.
    matcher.captures.write(slot, previous);
    return null;
  }
  final result = _Sequence(
    alternatives[alternativeIndex],
    0,
    matcher,
    _CaptureEnd(slot, matcher, next, position),
  ).run(position);
  if (result != null) return result;
  // Undo the span the failed alternative wrote so later backtracking sees the
  // state from before this group.
  matcher.captures.write(slot, previous);
  return _tryCapturingFrom(
    alternatives,
    alternativeIndex + 1,
    slot,
    previous,
    matcher,
    next,
    position,
  );
}

/// Writes the capture span, then resumes [next]; restores the previous span
/// when the rest of the match fails.
class _CaptureEnd extends _MatchContinuation {
  final int slot;
  final _RegexpMatcher matcher;
  final _MatchContinuation next;
  final int groupStart;

  _CaptureEnd(this.slot, this.matcher, this.next, this.groupStart);

  @override
  int? run(int position) {
    // Pack as start | (end << 20), matching [EmbedderRegexpMatch.group].
    final packed =
        (groupStart & 0xFFFFF) | ((position & 0xFFFFF) << 20);
    final previous = matcher.captures.readUnsigned(slot) & 0xFFFFFFFF;
    matcher.captures.write(slot, packed);
    final result = next.run(position);
    if (result == null) {
      matcher.captures.write(slot, previous);
    }
    return result;
  }
}

class _GroupEnd extends _MatchContinuation {
  final _MatchContinuation next;
  _GroupEnd(this.next);

  @override
  int? run(int position) => next.run(position);
}

/// `atom{min,max}` (`max < 0`: unbounded), greedy or lazy.
final class _QuantifierNode extends _RegexpNode {
  final _RegexpNode atom;
  final int min;
  final int max;
  final bool lazy;

  _QuantifierNode(this.atom, this.min, this.max, this.lazy)
      : assert(min >= 0),
        assert(max < 0 || max >= min);

  @override
  bool get canMatchEmpty => min == 0 || atom.canMatchEmpty;

  @override
  int? matchAt(int position, _RegexpMatcher matcher, _MatchContinuation next) {
    if (lazy) {
      return _repeatLazy(atom, min, max, matcher, next, position, 0);
    }
    return _repeatGreedy(atom, min, max, matcher, next, position, 0);
  }
}

int? _repeatGreedy(
  _RegexpNode atom,
  int min,
  int max,
  _RegexpMatcher matcher,
  _MatchContinuation next,
  int position,
  int count,
) {
  if (max < 0 || count < max) {
    final result = _Sequence(
      [atom],
      0,
      matcher,
      _GreedyContinue(atom, min, max, matcher, next, count),
    ).run(position);
    if (result != null) return result;
  }
  if (count >= min) return next.run(position);
  return null;
}

class _GreedyContinue extends _MatchContinuation {
  final _RegexpNode atom;
  final int min;
  final int max;
  final _RegexpMatcher matcher;
  final _MatchContinuation next;
  final int count;

  _GreedyContinue(
    this.atom,
    this.min,
    this.max,
    this.matcher,
    this.next,
    this.count,
  );

  @override
  int? run(int position) {
    return _repeatGreedy(atom, min, max, matcher, next, position, count + 1);
  }
}

int? _repeatLazy(
  _RegexpNode atom,
  int min,
  int max,
  _RegexpMatcher matcher,
  _MatchContinuation next,
  int position,
  int count,
) {
  if (count >= min) {
    final result = next.run(position);
    if (result != null) return result;
  }
  if (max < 0 || count < max) {
    final result = _Sequence(
      [atom],
      0,
      matcher,
      _LazyContinue(atom, min, max, matcher, next, count),
    ).run(position);
    if (result != null) return result;
  }
  return null;
}

class _LazyContinue extends _MatchContinuation {
  final _RegexpNode atom;
  final int min;
  final int max;
  final _RegexpMatcher matcher;
  final _MatchContinuation next;
  final int count;

  _LazyContinue(
    this.atom,
    this.min,
    this.max,
    this.matcher,
    this.next,
    this.count,
  );

  @override
  int? run(int position) {
    return _repeatLazy(atom, min, max, matcher, next, position, count + 1);
  }
}

/// `\1` .. `\99`: matches the same text as the referenced group.
final class _BackreferenceNode extends _RegexpNode {
  final int groupIndex;

  _BackreferenceNode(this.groupIndex);

  @override
  bool get canMatchEmpty => true;

  @override
  int? matchAt(int position, _RegexpMatcher matcher, _MatchContinuation next) {
    final packed = matcher.captures.readUnsigned(groupIndex - 1) & 0xFFFFFFFF;
    if (packed == 0) return next.run(position); // unset: matches empty
    final begin = packed & 0xFFFFF;
    final finish = (packed >>> 20) & 0xFFFFF;
    var offset = position;
    for (var i = begin; i < finish; i++) {
      if (!matcher.codeUnitEqualsAt(
        offset,
        matcher.string.codeUnitAtUnchecked(i),
      )) {
        return null;
      }
      offset++;
    }
    return next.run(offset);
  }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

class _RegexpParser {
  final String source;
  final bool unicode;
  final bool caseInsensitive;
  final bool dotAll;
  final bool multiLine;

  int position = 0;

  /// Number of capturing groups found so far, plus 1 for group 0 (the whole
  /// match). The Dart-visible `RegExpMatch.groupCount` excludes group 0, so
  /// the SDK export reports [groupCount] - 1.
  int groupCount = 1;
  final List<String?> groupNames = <String?>[null];
  final Map<String, int> groupIndicesByName = <String, int>{};
  bool isSticky = false;
  bool isGlobal = false;

  _RegexpParser({
    required this.source,
    required this.unicode,
    required this.caseInsensitive,
    required this.dotAll,
    required this.multiLine,
  });

  bool get atEnd => position >= source.length;

  int peek() => position < source.length ? source.codeUnitAt(position) : -1;

  int next() {
    if (atEnd) fail('Unexpected end of pattern');
    return source.codeUnitAt(position++);
  }

  Never fail(String message) => throw FormatException(message);

  /// Parses the top-level alternatives; the source must be fully consumed.
  List<List<_RegexpNode>> parseAlternatives() {
    final alternatives = <List<_RegexpNode>>[];
    for (;;) {
      final nodes = <_RegexpNode>[];
      while (peek() != -1 && peek() != 0x29 && peek() != 0x7c) {
        nodes.add(parseQuantified());
      }
      alternatives.add(nodes);
      if (peek() == 0x7c) {
        position++; // `|`
        continue;
      }
      if (atEnd) return alternatives;
      fail('Unmatched )');
    }
  }

  /// Parses one atom followed by at most one quantifier (with optional `?`).
  _RegexpNode parseQuantified() {
    final atom = parseAtom();
    final quantifier = _tryParseQuantifier();
    if (quantifier == null) return atom;
    var node = _QuantifierNode(atom, quantifier.$1, quantifier.$2, false);
    if (peek() == 0x3f) {
      position++;
      node = _QuantifierNode(atom, quantifier.$1, quantifier.$2, true);
    }
    return node;
  }

  (int, int)? _tryParseQuantifier() {
    switch (peek()) {
      case 0x2a: // *
        position++;
        return (0, -1);
      case 0x2b: // +
        position++;
        return (1, -1);
      case 0x3f: // ?
        position++;
        return (0, 1);
      case 0x7b: // {n}, {n,}, {n,m} — otherwise a literal brace
        return _tryParseCounted();
      default:
        return null;
    }
  }

  (int, int)? _tryParseCounted() {
    final saved = position;
    position++; // `{`
    final min = _parseDecimal();
    if (min < 0) {
      position = saved;
      return null; // not a quantifier: literal `{`
    }
    var max = min;
    if (peek() == 0x2c) {
      position++;
      max = -1;
      if (peek() >= 0x30 && peek() <= 0x39) {
        max = _parseDecimal();
      }
    }
    if (peek() != 0x7d) {
      position = saved;
      return null; // literal `{`
    }
    position++;
    if (max >= 0 && max < min) fail('numbers out of order in {} quantifier');
    return (min, max);
  }

  int _parseDecimal() {
    var value = 0;
    var digits = 0;
    while (peek() >= 0x30 && peek() <= 0x39) {
      value = value * 10 + (next() - 0x30);
      digits++;
      if (value > 0xFFFFF) value = 0xFFFFF;
    }
    return digits == 0 ? -1 : value;
  }

  /// Parses one atom (no quantifier).
  _RegexpNode parseAtom() {
    switch (peek()) {
      case 0x5e: // ^
        position++;
        return _AnchorNode(false, multiLine);
      case 0x24: // $
        position++;
        return _AnchorNode(true, multiLine);
      case 0x5c: // \
        position++;
        return parseEscape(inClass: false);
      case 0x28: // (
        position++;
        return parseGroup();
      case 0x5b: // [
        position++;
        return parseClass();
      case 0x2e: // .
        position++;
        return _DotNode(dotAll);
      case 0x2a || 0x2b || 0x3f: // * + ?
        fail('Nothing to repeat');
      case 0x7b: // `{` — a quantifier without an atom, or a literal brace
        final counted = _tryParseCounted();
        if (counted != null) fail('Nothing to repeat');
        position++;
        return _CharNode(0x7b, caseInsensitive);
      case 0x29:
        fail('Unmatched )');
      case -1:
        fail('Unexpected end of pattern');
    }
    final code = next();
    return _charNodeForCodePoint(code, caseInsensitive, unicode);
  }

  /// Parses a group starting after `(`.
  _RegexpNode parseGroup() {
    var kind = 1; // capturing
    var captureIndex = 0;
    String? name;
    if (peek() == 0x3f) {
      position++;
      switch (peek()) {
        case 0x3a: // (?:
          position++;
          kind = 0;
        case 0x3d: // (?=
          position++;
          kind = 2;
        case 0x21: // (?!
          position++;
          kind = 3;
        case 0x3c: // (?<name>…>  (lookbehind is not supported)
          position++;
          if (peek() == 0x3d || peek() == 0x21) {
            fail('Lookbehind assertions are not supported');
          }
          name = parseGroupName();
          kind = 1;
        default:
          fail('Invalid group');
      }
    }
    if (kind == 1) {
      captureIndex = groupCount++;
      groupNames.add(name);
      if (name != null) groupIndicesByName[name] = captureIndex;
    }
    final alternatives = parseGroupAlternatives();
    return _GroupNode(alternatives, captureIndex, kind);
  }

  /// Parses `alternatives)` inside a group.
  List<List<_RegexpNode>> parseGroupAlternatives() {
    final alternatives = <List<_RegexpNode>>[];
    for (;;) {
      final nodes = <_RegexpNode>[];
      while (peek() != -1 && peek() != 0x29 && peek() != 0x7c) {
        nodes.add(parseQuantified());
      }
      alternatives.add(nodes);
      if (peek() == 0x29) {
        position++;
        return alternatives;
      }
      if (atEnd) fail('Unterminated group');
      position++; // `|`
    }
  }

  String parseGroupName() {
    // Read the raw name up to `>`; like the VM, accept any characters rather
    // than validating the JS IdentifierStart/IdentifierPart grammar.
    final start = position;
    while (peek() != 0x3e && peek() != -1) {
      position++;
    }
    if (atEnd) fail('Invalid capture group name');
    final name = source.substring(start, position);
    position++; // `>`
    if (name.isEmpty) fail('Invalid capture group name');
    if (groupIndicesByName.containsKey(name)) {
      fail('Duplicate capture group name');
    }
    return name;
  }

  /// Parses `\...` with the backslash already consumed.
  _RegexpNode parseEscape({required bool inClass}) {
    if (atEnd) fail('\\ at end of pattern');
    final code = next();
    switch (code) {
      case 0x62 when !inClass: // \b: word boundary
        return _WordBoundaryNode(false);
      case 0x42 when !inClass: // \B
        return _WordBoundaryNode(true);
      case 0x62: // \b inside a class: backspace
        return _CharNode(0x08, caseInsensitive);
      case 0x42:
        return _CharNode(0x42, caseInsensitive);
      case 0x66:
        return _CharNode(0x0c, caseInsensitive);
      case 0x6e:
        return _CharNode(0x0a, caseInsensitive);
      case 0x72:
        return _CharNode(0x0d, caseInsensitive);
      case 0x74:
        return _CharNode(0x09, caseInsensitive);
      case 0x76:
        return _CharNode(0x0b, caseInsensitive);
      case 0x64 || 0x44 || 0x77 || 0x57 || 0x73 || 0x53:
        if (inClass) fail('Class escapes in classes are not supported yet');
        return _BuiltinClassNode(code);
      case 0x30:
        if (peek() >= 0x30 && peek() <= 0x39) {
          fail('Invalid octal escape');
        }
        return _CharNode(0, caseInsensitive);
      case 0x63: // \cX: the control letter mod 32
        final control = peek();
        if (control >= 0x41 && control <= 0x7a) {
          position++;
          return _CharNode(control & 0x1f, caseInsensitive);
        }
        fail('Invalid \\c escape');
      case 0x78:
        return _charNodeForCodePoint(readHex(2), caseInsensitive, unicode);
      case 0x75:
        return parseUnicodeEscape();
      default:
        if (code >= 0x31 && code <= 0x39) {
          if (inClass) fail('Invalid class escape');
          final reference = readBackreference(code - 0x30);
          if (reference >= groupCount) fail('Invalid backreference');
          return _BackreferenceNode(reference);
        }
        return _CharNode(code, caseInsensitive);
    }
  }

  int readBackreference(int firstDigit) {
    var value = firstDigit;
    while (peek() >= 0x30 && peek() <= 0x39) {
      final candidate = value * 10 + (peek() - 0x30);
      if (candidate >= groupCount) break;
      position++;
      value = candidate;
    }
    return value;
  }

  _RegexpNode parseUnicodeEscape() {
    if (peek() == 0x7b) {
      if (!unicode) fail('Invalid unicode escape');
      position++;
      var value = 0;
      var any = false;
      while (peek() != 0x7d && !atEnd) {
        value = value * 16 + hexValue(next());
        any = true;
        if (value > 0x10FFFF) fail('Invalid unicode escape');
      }
      if (!any || atEnd) fail('Invalid unicode escape');
      position++; // `}`
      return _charNodeForCodePoint(value, caseInsensitive, unicode);
    }
    return _charNodeForCodePoint(readHex(4), caseInsensitive, unicode);
  }

  /// Parses a character class; the `[` is already consumed.
  _RegexpNode parseClass() {
    var negated = false;
    if (peek() == 0x5e) {
      position++;
      negated = true;
    }
    final ranges = <(int, int)>[];
    var first = true;
    while (peek() != 0x5d || first) {
      if (atEnd) fail('Unterminated character class');
      first = false;
      parseClassAtom(ranges);
    }
    position++; // `]`
    ranges.sort((a, b) => a.$1.compareTo(b.$1));
    return _ClassNode(ranges, negated);
  }

  void parseClassAtom(List<(int, int)> ranges) {
    final startCode = parseClassAtomCode();
    if (peek() == 0x2d &&
        position + 1 < source.length &&
        source.codeUnitAt(position + 1) != 0x5d) {
      position++; // `-`
      final endCode = parseClassAtomCode();
      if (endCode < startCode) fail('Range out of order in character class');
      ranges.add((startCode, endCode));
    } else {
      ranges.add((startCode, startCode));
    }
  }

  int parseClassAtomCode() {
    final code = next();
    if (code != 0x5c) return code;
    if (atEnd) fail('\\ at end of pattern');
    final escaped = next();
    switch (escaped) {
      case 0x62:
        return 0x08;
      case 0x66:
        return 0x0c;
      case 0x6e:
        return 0x0a;
      case 0x72:
        return 0x0d;
      case 0x74:
        return 0x09;
      case 0x76:
        return 0x0b;
      case 0x30:
        if (peek() >= 0x30 && peek() <= 0x39) {
          fail('Invalid octal escape');
        }
        return 0;
      case 0x63:
        final control = peek();
        if (control >= 0x41 && control <= 0x7a) {
          position++;
          return control & 0x1f;
        }
        fail('Invalid \\c escape');
      case 0x78:
        return readHex(2);
      case 0x75:
        if (peek() == 0x7b) {
          if (!unicode) fail('Invalid unicode escape');
          position++;
          var value = 0;
          var any = false;
          while (peek() != 0x7d && !atEnd) {
            value = value * 16 + hexValue(next());
            any = true;
            if (value > 0x10FFFF) fail('Invalid unicode escape');
          }
          if (!any || atEnd) fail('Invalid unicode escape');
          position++;
          return value;
        }
        return readHex(4);
      case 0x64 || 0x44 || 0x77 || 0x57 || 0x73 || 0x53:
        fail('Class escapes in classes are not supported yet');
      default:
        return escaped;
    }
  }

  int readHex(int digits) {
    var value = 0;
    for (var i = 0; i < digits; i++) {
      if (atEnd) fail('Invalid hexadecimal escape');
      value = value * 16 + hexValue(next());
    }
    return value;
  }

  int hexValue(int code) {
    if (code >= 0x30 && code <= 0x39) return code - 0x30;
    if (code >= 0x41 && code <= 0x46) return code - 0x37;
    if (code >= 0x61 && code <= 0x66) return code - 0x57;
    fail('Invalid hexadecimal escape');
  }
}
