import 'string.dart';

import 'double_format.dart' show decimalFractionToDouble;

/// Result of [tryParseDouble]: either a double or a failure, handed to the
/// SDK as an opaque extern ref by [exports.dart].
sealed class DoubleParseResult {
  const DoubleParseResult();
}

final class DoubleParseSuccess extends DoubleParseResult {
  final double value;

  const DoubleParseSuccess(this.value);
}

final class DoubleParseFailure extends DoubleParseResult {
  const DoubleParseFailure();
}

/// Implements [double.tryParse] / [double.parse] semantics
/// (https://github.com/dart-lang/sdk/blob/main/sdk/lib/core/double.dart):
///
///   whitespace* sign digits ('.' digits)? exponent? whitespace*
///   exponent := ('e' | 'E') sign? digits
///
/// An empty fraction part is *not* allowed ('1.' fails), and the whole
/// string must be consumed. This is stricter than JavaScript's parseFloat
/// and matches the VM.
DoubleParseResult tryParseDouble(WasmStringImplementation source) {
  final length = source.length;
  var index = 0;

  // Leading whitespace.
  while (index < length && _isWhitespace(source.codeUnitAtUnchecked(index))) {
    index++;
  }

  if (index == length) return const DoubleParseFailure();

  // Special values: exactly 'NaN' or [+-]'Infinity' (case sensitive),
  // optionally surrounded by whitespace.
  final special = _matchSpecialValue(source, index);
  if (special != null) {
    return special;
  }

  var negative = false;
  if (index < length) {
    final c = source.codeUnitAtUnchecked(index);
    if (c == 0x2b) {
      index++;
    } else if (c == 0x2d) {
      negative = true;
      index++;
    }
  }

  final intStart = index;
  var intDigits = 0;
  while (index < length && _isDigit(source.codeUnitAtUnchecked(index))) {
    index++;
    intDigits++;
  }

  var fracDigits = 0;
  var fracStart = index;
  if (index < length && source.codeUnitAtUnchecked(index) == 0x2e) {
    index++;
    fracStart = index;
    while (index < length && _isDigit(source.codeUnitAtUnchecked(index))) {
      index++;
      fracDigits++;
    }
    if (intDigits == 0 && fracDigits == 0) {
      // Just '.', or '.e5' - not a number.
      return const DoubleParseFailure();
    }
    // A trailing dot with no fraction digits is allowed ('1.' parses; so
    // does '1.e5'). With nothing at all after the dot ('1.5.'), the
    // trailing '.' can not start a new number, so the whole-string check
    // below rejects it. Back up only when no digits and no exponent follow,
    // i.e. when the dot is the last character of a *valid* number prefix.
    if (fracDigits == 0 &&
        index < length &&
        source.codeUnitAtUnchecked(index) != 0x65 &&
        source.codeUnitAtUnchecked(index) != 0x45) {
      // Something follows that cannot continue the number: fail.
      return const DoubleParseFailure();
    }
  } else if (intDigits == 0) {
    // No digits at all.
    return const DoubleParseFailure();
  }

  var exponent = 0;
  var explicitExponentDigits = 0;
  if (index < length) {
    final c = source.codeUnitAtUnchecked(index);
    if (c == 0x65 || c == 0x45) {
      var eIndex = index + 1;
      var eNegative = false;
      if (eIndex < length) {
        final eChar = source.codeUnitAtUnchecked(eIndex);
        if (eChar == 0x2b) {
          eIndex++;
        } else if (eChar == 0x2d) {
          eNegative = true;
          eIndex++;
        }
      }
      final eDigitsStart = eIndex;
      while (eIndex < length && _isDigit(source.codeUnitAtUnchecked(eIndex))) {
        eIndex++;
      }
      if (eIndex == eDigitsStart) {
        // 'e' without digits: the exponent marker is not consumed, so the
        // remaining text fails the "whole string" requirement.
        return const DoubleParseFailure();
      }
      exponent = _parseSmallInt(
        source,
        eDigitsStart,
        eIndex,
        eNegative,
      );
      explicitExponentDigits = eIndex - eDigitsStart;
      index = eIndex;
    }
  }

  // Trailing whitespace.
  while (index < length && _isWhitespace(source.codeUnitAtUnchecked(index))) {
    index++;
  }
  if (index != length) {
    // Garbage after the number.
    return const DoubleParseFailure();
  }

  // Collect the significant digits (dropping the decimal point) and track
  // the decimal exponent adjustment from the fraction part.
  final digitBuffer = <int>[];
  var valueExponent = exponent;
  for (var i = intStart; i < intStart + intDigits; i++) {
    digitBuffer.add(source.codeUnitAtUnchecked(i) - 0x30);
  }
  if (fracDigits > 0) {
    for (var i = fracStart; i < fracStart + fracDigits; i++) {
      digitBuffer.add(source.codeUnitAtUnchecked(i) - 0x30);
    }
    valueExponent -= fracDigits;
  }

  // Strip leading zeros (they do not change the value).
  var start = 0;
  while (start < digitBuffer.length && digitBuffer[start] == 0) {
    start++;
  }
  // Strip trailing zeros, adjusting the exponent so the value is unchanged.
  var end = digitBuffer.length;
  while (end > start && digitBuffer[end - 1] == 0) {
    end--;
    valueExponent++;
  }
  final significant = digitBuffer.sublist(start, end);

  double magnitude;
  if (significant.isEmpty) {
    magnitude = 0.0;
  } else if (explicitExponentDigits > 8) {
    // Huge explicit exponents would blow up the pow10 tables; clamp to the
    // double range instead.
    magnitude = _clampToInfinity(valueExponent);
  } else if (valueExponent > 400 || valueExponent + significant.length > 420) {
    magnitude = double.infinity;
  } else if (valueExponent + significant.length < -420) {
    magnitude = 0.0;
  } else {
    magnitude = decimalFractionToDouble(
      _bigFromDigits(significant),
      valueExponent,
    );
  }

  if (negative) {
    magnitude = -magnitude;
  }
  return DoubleParseSuccess(magnitude);
}

/// Exponents beyond the double range with unrepresentable magnitude: ±0 or
/// ±Infinity. (No rounding subtleties: anything this extreme is monotone.)
double _clampToInfinity(int exponent) {
  return exponent >= 0 ? double.infinity : 0.0;
}

BigInt _bigFromDigits(List<int> digits) {
  var result = BigInt.zero;
  for (final d in digits) {
    result = result * _bigTen + BigInt.from(d);
  }
  return result;
}

final _bigTen = BigInt.from(10);

int _parseSmallInt(
  WasmStringImplementation source,
  int start,
  int end,
  bool negative,
) {
  var value = 0;
  for (var i = start; i < end; i++) {
    final digit = source.codeUnitAtUnchecked(i) - 0x30;
    if (value < 0x10000000) {
      value = value * 10 + digit;
    } else {
      // Saturate; the magnitude clamping handles the rest.
      value = negative ? -0x7fffffff : 0x7fffffff;
    }
  }
  return negative ? -value : value;
}

bool _isDigit(int code) => code >= 0x30 && code <= 0x39;

/// Matches 'NaN' or [+-]'Infinity' (case sensitive) at [start], with only
/// whitespace allowed to follow. Returns the value or null.
DoubleParseSuccess? _matchSpecialValue(
  WasmStringImplementation source,
  int start,
) {
  final length = source.length;
  var index = start;
  var negative = false;
  final first = source.codeUnitAtUnchecked(index);
  if (first == 0x2b) {
    index++;
  } else if (first == 0x2d) {
    negative = true;
    index++;
  }

  bool matchesWord(String word) {
    if (index + word.length > length) return false;
    for (var i = 0; i < word.length; i++) {
      if (source.codeUnitAtUnchecked(index + i) != word.codeUnitAt(i)) {
        return false;
      }
    }
    var end = index + word.length;
    while (end < length && _isWhitespace(source.codeUnitAtUnchecked(end))) {
      end++;
    }
    return end == length;
  }

  if (matchesWord('NaN')) {
    return const DoubleParseSuccess(double.nan);
  }
  if (matchesWord('Infinity')) {
    return DoubleParseSuccess(negative ? double.negativeInfinity : double.infinity);
  }
  return null;
}

bool _isWhitespace(int code) {
  // ECMAScript whitespace accepted by double.parse: space, tab, LF, VT, FF,
  // CR, plus NBSP-ish Dart additions (0x85, 0xa0, and Unicode spaces).
  switch (code) {
    case 0x09:
    case 0x0a:
    case 0x0b:
    case 0x0c:
    case 0x0d:
    case 0x20:
    case 0x85:
    case 0xa0:
    case 0x1680:
    case 0x2000:
    case 0x2001:
    case 0x2002:
    case 0x2003:
    case 0x2004:
    case 0x2005:
    case 0x2006:
    case 0x2007:
    case 0x2008:
    case 0x2009:
    case 0x200a:
    case 0x2028:
    case 0x2029:
    case 0x202f:
    case 0x205f:
    case 0x3000:
    case 0xfeff:
      return true;
    default:
      return false;
  }
}
