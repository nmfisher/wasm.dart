// ignore: import_internal_library
import 'dart:_wasm';
import 'dart:typed_data';

import 'string.dart';

/// Exact double formatting on top of [BigInt].
///
/// The SDK has no primitive double formatter - the `f64To*` imports exist
/// precisely because of that - so the digits are generated here with exact
/// integer arithmetic instead of calling back into `toStringAs*`, which would
/// recurse into the very imports implemented here.
///
/// All functions produce the same output as their JavaScript counterparts,
/// which is what the SDK contract requires (see `boxed_double_patch.dart` in
/// the SDK).
///
/// Rounding contract (verified against the Dart VM / V8): ties on the exact
/// binary value round away from zero, e.g. `(2.5).toStringAsFixed(0) == '3'`
/// and `(0.25).toStringAsPrecision(1) == '0.3'` while
/// `(0.15).toStringAsPrecision(1) == '0.1'` (the double nearest 0.15 is just
/// below the tie).

final Float64List _floatView = Float64List(1);
final Uint32List _bitsView = _floatView.buffer.asUint32List();

int _doubleBits(double value) {
  _floatView[0] = value;
  // Typed data views are little-endian on wasm.
  return (_bitsView[1] << 32) | _bitsView[0];
}

double _bitsToDouble(int bits) {
  _bitsView[0] = bits & 0xffffffff;
  _bitsView[1] = (bits >>> 32) & 0xffffffff;
  return _floatView[0];
}

/// Decomposed double: `mantissa * 2^exponent` with a positive, integral
/// mantissa of at most 53 bits.
final class _Decomposed {
  final BigInt mantissa;
  final int exponent;

  _Decomposed(this.mantissa, this.exponent);
}

_Decomposed _decompose(double value) {
  final bits = _doubleBits(value);
  final rawExponent = (bits >>> 52) & 0x7ff;
  // Only the 52 fraction bits belong to the mantissa.
  final fraction = BigInt.from(bits & 0xfffffffffffff);
  if (rawExponent == 0) {
    // Subnormal (or zero).
    return _Decomposed(fraction, -1074);
  }
  // Normal: add the implicit leading one and remove the bias.
  return _Decomposed(fraction | (BigInt.one << 52), rawExponent - 1075);
}

/// Formats like [double.toString].
Latin1String doubleToString(double value) {
  if (value.isNaN) return _nan;
  if (value == double.infinity) return _infinity;
  if (value == -double.infinity) return _negativeInfinity;
  if (value == 0) return value.isNegative ? _negativeZero : _zero;

  final digits = _shortestDigits(value);
  final decimalExponent = digits.exponent + digits.count - 1;
  final negative = value.isNegative;

  if (decimalExponent >= 21 || decimalExponent <= -7) {
    final sign = negative ? '-' : '';
    final exponentSign = decimalExponent >= 0 ? '+' : '';
    return _latin1(
      '$sign${_exponentialBody(digits)}e$exponentSign$decimalExponent',
    );
  }
  return _latin1(_plainString(digits, negative, dartZeroSuffix: true));
}

/// Formats like `Number.prototype.toExponential` and
/// [double.toStringAsExponential]. A negative [fractionDigits] means the
/// shortest representation is used (that is how the SDK's
/// `Double.toStringAsExponential()` calls the import).
Latin1String doubleToExponentialWithFractionDigits(
  double value,
  int fractionDigits,
) {
  return doubleToExponential(
    value,
    fractionDigits < 0 ? null : fractionDigits,
  );
}

/// Shortest-exponential variant of [doubleToExponentialWithFractionDigits].
Latin1String doubleToExponential(double value, int? fractionDigits) {
  if (value.isNaN) return _nan;
  if (value == double.infinity) return _infinity;
  if (value == -double.infinity) return _negativeInfinity;

  String mantissa;
  var exponent = 0;
  if (value == 0) {
    if (value.isNegative) {
      mantissa = '-0';
    } else {
      mantissa = '0';
    }
    if (fractionDigits != null && fractionDigits > 0) {
      mantissa = '$mantissa.${'0' * fractionDigits}';
    }
  } else {
    final digits = fractionDigits == null
        ? _shortestDigits(value)
        : _roundedDigits(value, fractionDigits + 1);
    mantissa = digits.mantissaString(fractionDigits);
    if (value.isNegative) mantissa = '-$mantissa';
    exponent = digits.exponent + digits.count - 1;
  }
  final sign = exponent >= 0 ? '+' : '';
  return _latin1('${mantissa}e$sign$exponent');
}

/// Formats like `Number.prototype.toPrecision`.
Latin1String doubleToPrecision(double value, int precision) {
  if (value.isNaN) return _nan;
  if (value == double.infinity) return _infinity;
  if (value == -double.infinity) return _negativeInfinity;

  final digits = _roundedDigits(value, precision);
  final decimalExponent = digits.exponent + digits.count - 1;
  final negative = value.isNegative;

  // JS uses plain notation when the decimal exponent is in (-7, precision).
  if (decimalExponent >= -7 && decimalExponent < precision) {
    return _latin1(_plainString(digits, negative, precision: precision));
  }
  var mantissa = digits.mantissaString(precision - 1);
  if (negative) mantissa = '-$mantissa';
  final sign = decimalExponent >= 0 ? '+' : '';
  return _latin1('${mantissa}e$sign$decimalExponent');
}

/// Formats like `Number.prototype.toFixed`.
Latin1String doubleToFixed(double value, int fractionDigits) {
  if (value.isNaN) return _nan;
  if (value == double.infinity) return _infinity;
  if (value == -double.infinity) return _negativeInfinity;

  // ECMAScript step 3: |value| >= 1e21 behaves like toString (no fixed).
  if (value.abs() >= 1e21) {
    return doubleToString(value);
  }

  // toFixed rounds at the *fraction place*, not at a significant-digit
  // count, so compute the integer n closest to x * 10^f (ties away from
  // zero) directly on the exact binary value.
  final negative = value.isNegative;
  final abs = value.abs();
  final d = _decompose(abs);
  // n = closest integer to abs * 10^fractionDigits. A rounding rollover
  // (9.99 -> 10.0) simply grows the digit string, which the point
  // arithmetic absorbs.
  final scaled = _roundDecimal(d.mantissa, d.exponent, fractionDigits);
  final text = scaled.toString();
  final intDigits = text.length - fractionDigits;
  String result;
  if (intDigits > 0) {
    result = '${text.substring(0, intDigits)}.${text.substring(intDigits)}';
  } else {
    // |value| < 1: leading fraction zeros.
    result = '0.${'0' * -intDigits}$text';
  }
  if (negative) result = '-$result';
  return _latin1(result);
}

final class _Digits {
  /// Significant decimal digits, most significant first, never empty.
  final String digits;

  /// The value is `0.digits * 10^(exponent + count)`.
  final int exponent;

  _Digits(this.digits, this.exponent);

  int get count => digits.length;

  /// The mantissa in JS form: `d` or `d.ddd`, with exactly [maxFraction]
  /// fraction digits when given.
  String mantissaString([int? maxFraction]) {
    var rest = count > 1 ? digits.substring(1) : '';
    if (maxFraction != null) {
      if (rest.length > maxFraction) {
        rest = rest.substring(0, maxFraction);
      } else {
        rest = rest.padRight(maxFraction, '0');
      }
    }
    if (rest.isEmpty) return digits.substring(0, 1);
    return '${digits.substring(0, 1)}.$rest';
  }
}

/// Shortest digit string that round-trips `value` exactly, as JS requires.
_Digits _shortestDigits(double value) {
  // Digits are generated from the magnitude; compare magnitudes as well.
  // Ties break to even here (Grisu/Ryu-style shortest modes pick the even
  // digit, e.g. 2056015024933716.2 whose 17-digit form is an exact tie),
  // while fixed-precision modes round ties away from zero.
  final abs = value.abs();
  for (var precision = 1; precision <= 16; precision++) {
    final digits = _roundedDigits(abs, precision, tiesEven: true);
    if (_digitsToDouble(digits).compareTo(abs) == 0) {
      return digits;
    }
  }
  // 17 digits always round-trip for IEEE doubles; trailing zeros are then
  // stripped to match the shortest representation (this is also what makes
  // values like 1e23 print as `1e+23` rather than `9.99...e+22`).
  final digits = _roundedDigits(abs, 17, tiesEven: true);
  var text = digits.digits;
  var count = text.length;
  final originalCount = count;
  while (count > 1 && text.endsWith('0')) {
    count--;
    text = text.substring(0, count);
  }
  // Keep the value identical: 0.digits * 10^(exponent + count).
  return _Digits(
    text,
    digits.exponent + originalCount - count,
  );
}

/// Parses `0.digits * 10^(exponent + count)` back into a double with
/// correct rounding (ties to even), using exact [BigInt] arithmetic.
double _digitsToDouble(_Digits digits) {
  // value = mantissa * 10^shift, as an exact fraction.
  final mantissa = _bigFromDecimalDigits(digits.digits);
  final shift = digits.exponent;

  var numerator = mantissa;
  var denominator = BigInt.one;
  if (shift >= 0) {
    numerator *= _pow10(shift);
  } else {
    denominator *= _pow10(-shift);
  }

  // Normalize to value = m * 2^-1074 with m integral: multiply by 2^1074,
  // then divide out the decimal denominator.
  final significand = _roundHalfEven(
    numerator << 1074,
    denominator,
  );
  if (significand == BigInt.zero) {
    return 0.0;
  }

  // significand = value * 2^1074. The double itself is the 53-bit rounding
  // of this number (with ties to even, as a correct decimal parser rounds).
  var m = significand;
  var length = m.bitLength;
  if (length > 53) {
    m = _roundHalfEven(m, BigInt.one << (length - 53));
    if (m.bitLength > 53) {
      // Rounding carried into the next binade (e.g. 0.999... -> 1.0).
      m >>= 1;
      length++;
    }
  }
  if (length > 53) {
    // Normal: m is the 53-bit significand of
    // value = m * 2^(length - 53 - 1074) = 1.f * 2^(length - 1075).
    final biased = length - 1075 + 1023;
    return _bitsToDouble(
      (biased << 52) | _bigToInt(m - (BigInt.one << 52)),
    );
  }
  // Subnormal: value = m * 2^-1074 with m < 2^52; the significand is the
  // fraction field itself.
  return _bitsToDouble(_bigToInt(m));
}

/// `numerator ~/ denominator`, rounded to the nearest integer, ties to even.
/// Both operands must be positive.
BigInt _roundHalfEven(BigInt numerator, BigInt denominator) {
  assert(denominator > BigInt.zero);
  final quotient = numerator ~/ denominator;
  final remainder = numerator - quotient * denominator;
  final twiceRemainder = remainder << 1;
  if (twiceRemainder > denominator ||
      (twiceRemainder == denominator && quotient.isOdd)) {
    return quotient + BigInt.one;
  }
  return quotient;
}

/// Parses a string of decimal digits into a [BigInt] without going through
/// `BigInt.parse` (which drags `RegExp` support into the component).
BigInt _bigFromDecimalDigits(String digits) {
  var result = BigInt.zero;
  final ten = BigInt.from(10);
  for (var i = 0; i < digits.length; i++) {
    result = result * ten + BigInt.from(digits.codeUnitAt(i) - 0x30);
  }
  return result;
}

int _bigToInt(BigInt value) {
  var result = 0;
  for (var i = 0; i < 64 && value > BigInt.zero; i++) {
    if ((value & BigInt.one) == BigInt.one) {
      result |= 1 << i;
    }
    value >>= 1;
  }
  return result;
}

/// The first [precision] significant decimal digits of `value`, correctly
/// rounded. Ties on the exact binary value round away from zero, unless
/// [tiesEven] is set (shortest-round-trip mode matches the even-digit
/// choice of Grisu/Ryu).
_Digits _roundedDigits(double value, int precision, {bool tiesEven = false}) {
  if (value == 0) return _Digits('0', 0);
  final d = _decompose(value);
  final mantissa = d.mantissa;
  final exponent = d.exponent;

  // floor(log10(value)): value = mantissa * 2^exponent, so
  // log10(value) = log10(mantissa) + exponent * log10(2). Estimate with
  // fixed point math (0x4d10/0x10000 ~ log10(2)), then correct with exact
  // integer comparisons.
  var estimate = ((mantissa.bitLength * 0x4d10) >> 16) +
      ((exponent * 0x4d10) >> 16);
  while (_atLeastPow10(mantissa, exponent, estimate + 1)) {
    estimate++;
  }
  while (!_atLeastPow10(mantissa, exponent, estimate)) {
    estimate--;
  }

  // Round value * 10^(precision - 1 - estimate) to the nearest integer.
  // value * 10^shift = mantissa * 2^exponent * 5^shift * 2^shift, so the
  // decimal shift becomes a 5^shift scaling plus a binary shift.
  // If rounding carries past [precision] digits (e.g. 9.99e2 -> 1e3), the
  // estimate itself was really one higher.
  var scaled = _roundDecimal(
    mantissa,
    exponent,
    precision - 1 - estimate,
    tiesEven: tiesEven,
  );
  if (scaled >= _pow10(precision)) {
    estimate++;
    scaled = _roundDecimal(
      mantissa,
      exponent,
      precision - 1 - estimate,
      tiesEven: tiesEven,
    );
  }
  var text = scaled.toString();
  if (text.length > precision) text = text.substring(0, precision);
  return _Digits(text, estimate - precision + 1);
}

/// `round(mantissa * 2^binaryExponent * 10^decimalShift)`, ties away from
/// zero (operands are positive) unless [tiesEven].
BigInt _roundDecimal(
  BigInt mantissa,
  int binaryExponent,
  int decimalShift, {
  bool tiesEven = false,
}) {
  final binaryShift = binaryExponent + decimalShift;
  if (decimalShift >= 0) {
    final value = mantissa * _pow5(decimalShift);
    if (binaryShift >= 0) return value << binaryShift;
    return _divPow2(value, -binaryShift, tiesEven: tiesEven);
  }
  final denominator = _pow5(-decimalShift);
  if (binaryShift >= 0) {
    return _divRound(mantissa << binaryShift, denominator, tiesEven: tiesEven);
  }
  return _divRound(
    mantissa,
    denominator << -binaryShift,
    tiesEven: tiesEven,
  );
}

/// `numerator ~/ denominator` rounded to the nearest integer, ties rounded
/// up (away from zero; operands are positive) unless [tiesEven].
BigInt _divRound(
  BigInt numerator,
  BigInt denominator, {
  bool tiesEven = false,
}) {
  final quotient = numerator ~/ denominator;
  final remainder = numerator - quotient * denominator;
  final twiceRemainder = remainder << 1;
  if (twiceRemainder > denominator ||
      (twiceRemainder == denominator &&
          (tiesEven ? quotient.isOdd : true))) {
    return quotient + BigInt.one;
  }
  return quotient;
}

/// `value >> shift` rounded to the nearest integer, ties rounded up
/// (away from zero; value is positive) unless [tiesEven].
BigInt _divPow2(BigInt value, int shift, {bool tiesEven = false}) {
  final quotient = value >> shift;
  final remainder = value - (quotient << shift);
  final isTie = remainder == BigInt.one << (shift - 1);
  if (remainder > BigInt.one << (shift - 1) ||
      (isTie && (tiesEven ? quotient.isOdd : true))) {
    return quotient + BigInt.one;
  }
  return quotient;
}

/// Whether `mantissa * 2^exponent >= 10^decimalExponent`, exactly.
bool _atLeastPow10(BigInt mantissa, int exponent, int decimalExponent) {
  if (exponent >= 0) {
    return (mantissa << exponent) >= _pow10(decimalExponent);
  }
  // 2^-exponent = 5^-exponent / 10^-exponent.
  return (mantissa * _pow5(-exponent)) >=
      _pow10(decimalExponent - exponent);
}

final Map<int, BigInt> _pow10Cache = {};
final Map<int, BigInt> _pow5Cache = {};

BigInt _pow10(int exponent) {
  if (exponent < 0) return BigInt.one;
  return _pow10Cache.putIfAbsent(
    exponent,
    () => BigInt.from(10).pow(exponent),
  );
}

BigInt _pow5(int exponent) {
  if (exponent < 0) return BigInt.one;
  return _pow5Cache.putIfAbsent(
    exponent,
    () => BigInt.from(5).pow(exponent),
  );
}

/// The `d.ddd` body of an exponential rendering (no sign, no exponent).
String _exponentialBody(_Digits digits) {
  final buffer = StringBuffer();
  buffer.write(digits.digits[0]);
  if (digits.count > 1) {
    buffer
      ..write('.')
      ..write(digits.digits.substring(1));
  }
  return buffer.toString();
}

/// Renders the digits in JS plain notation. [precision] (from
/// `toPrecision`) pads with trailing zeros to exactly that many significant
/// digits and forces exponential rendering for large exponents; without it
/// (toString / toExponential cutoffs) the shortest digit string is used.
/// [dartZeroSuffix] appends `.0` to purely integral results, matching the
/// Dart VM's `double.toString` (`1.0` where JS prints `1`).
String _plainString(
  _Digits digits,
  bool negative, {
  int? precision,
  bool dartZeroSuffix = false,
}) {
  final buffer = StringBuffer();
  if (negative) buffer.write('-');

  var all = digits.digits;
  if (precision != null) {
    all = all.length >= precision
        ? all.substring(0, precision)
        : all.padRight(precision, '0');
  }

  final k = all.length;
  // The value is 0.all * 10^n.
  final n = digits.exponent + digits.count;

  if (k <= n && n <= 21) {
    // Purely integral, fits.
    buffer.write(all.padRight(n, '0'));
    if (dartZeroSuffix) buffer.write('.0');
  } else if (0 < n && n <= 21) {
    // Point inside the digits.
    buffer
      ..write(all.substring(0, n))
      ..write('.')
      ..write(all.substring(n));
  } else if (-6 < n && n <= 0) {
    // Fraction with leading zeros.
    buffer
      ..write('0.')
      ..write('0' * -n)
      ..write(all);
  } else if (precision != null) {
    // toPrecision with big |exponent|: exponential, padded to precision.
    buffer.write(all[0]);
    if (precision > 1) {
      buffer
        ..write('.')
        ..write(all.substring(1));
    }
    final e = n - 1;
    buffer
      ..write('e')
      ..write(e >= 0 ? '+' : '')
      ..write(e);
  } else {
    // toString / toExponential cutoffs: exponential, shortest digits.
    final e = n - 1;
    buffer
      ..write(all[0])
      ..write('.');
    buffer.write(k > 1 ? '${all.substring(1)}e' : 'e');
    buffer
      ..write(e >= 0 ? '+' : '')
      ..write(e);
  }
  return buffer.toString();
}

final _nan = _latin1('NaN');
final _infinity = _latin1('Infinity');
final _negativeInfinity = _latin1('-Infinity');
final _zero = _latin1('0.0');
final _negativeZero = _latin1('-0.0');

Latin1String _latin1(String text) {
  final array = WasmArray<WasmI8>(text.length);
  for (var i = 0; i < text.length; i++) {
    array.write(i, text.codeUnitAt(i));
  }
  return Latin1String.unsafeWrap(array);
}
