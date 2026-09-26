// ignore: import_internal_library
import 'dart:_wasm';

import 'string.dart';

/// Encodes [source] as a JSON string literal (including the surrounding
/// quotes), matching dart:convert's `_JsonStringifier.writeStringContent`:
///
/// - `"` becomes `\"`, `\` becomes `\\`
/// - `\b \t \n \f \r` use their short escapes
/// - other control characters become `\u00xx` with lowercase hex
/// - an unpaired surrogate becomes `\udXXX` (dart:convert escapes it rather
///   than emitting an invalid code unit)
/// - everything else (0x7f, paired surrogates, non-ASCII, U+2028/9) is
///   copied through unchanged
WasmStringImplementation jsonEncodeStringImpl(WasmStringImplementation source) {
  final length = source.length;

  // Size the output first so the result array is written exactly once.
  var extra = 0;
  for (var i = 0; i < length; i++) {
    final c = source.codeUnitAtUnchecked(i);
    if (c == 0x22 || c == 0x5c) {
      extra++;
    } else if (c < 0x20) {
      switch (c) {
        case 0x08:
        case 0x09:
        case 0x0a:
        case 0x0c:
        case 0x0d:
          extra++;
          break;
        default:
          extra += 5; // \u00xx
      }
    } else if (_isUnpairedSurrogateAt(source, i)) {
      extra += 5; // \udXXX
    }
  }

  final outLength = length + extra + 2;
  // Output is written as UTF-16 code units; for Latin-1 input the result is
  // still Latin-1 (escapes and passthroughs are all ASCII).
  if (source is Latin1String) {
    final out = WasmArray<WasmI8>(outLength);
    out.write(0, 0x22);
    var offset = 1;
    for (var i = 0; i < length; i++) {
      final code = source.codeUnitAtUnchecked(i);
      switch (code) {
        case 0x22:
          out
            ..write(offset++, 0x5c)
            ..write(offset++, 0x22);
        case 0x5c:
          out
            ..write(offset++, 0x5c)
            ..write(offset++, 0x5c);
        case 0x08:
          out
            ..write(offset++, 0x5c)
            ..write(offset++, 0x62);
        case 0x09:
          out
            ..write(offset++, 0x5c)
            ..write(offset++, 0x74);
        case 0x0a:
          out
            ..write(offset++, 0x5c)
            ..write(offset++, 0x6e);
        case 0x0c:
          out
            ..write(offset++, 0x5c)
            ..write(offset++, 0x66);
        case 0x0d:
          out
            ..write(offset++, 0x5c)
            ..write(offset++, 0x72);
        default:
          if (code < 0x20) {
            offset = _writeUnicodeEscapeI8(out, offset, code);
          } else {
            out.write(offset++, code);
          }
      }
    }
    out.write(offset, 0x22);
    return Latin1String.unsafeWrap(out);
  }
  final out = WasmArray<WasmI16>(outLength);
  out.write(0, 0x22);
  var offset = 1;
  for (var i = 0; i < length; i++) {
    offset = _writeEscaped(out, offset, source.codeUnitAtUnchecked(i), source, i);
  }
  out.write(offset, 0x22);
  return Utf16String.unsafeWrap(out);
}

/// JSON-escapes [code] into [out] at [offset]; returns the next offset.
/// [source] and [index] identify the surrounding string so an unpaired
/// surrogate can be escaped.
int _writeEscaped(
  WasmArray<WasmI16> out,
  int offset,
  int code,
  WasmStringImplementation source,
  int index,
) {
  switch (code) {
    case 0x22:
      out
        ..write(offset++, 0x5c)
        ..write(offset++, 0x22);
    case 0x5c:
      out
        ..write(offset++, 0x5c)
        ..write(offset++, 0x5c);
    case 0x08:
      out
        ..write(offset++, 0x5c)
        ..write(offset++, 0x62);
    case 0x09:
      out
        ..write(offset++, 0x5c)
        ..write(offset++, 0x74);
    case 0x0a:
      out
        ..write(offset++, 0x5c)
        ..write(offset++, 0x6e);
    case 0x0c:
      out
        ..write(offset++, 0x5c)
        ..write(offset++, 0x66);
    case 0x0d:
      out
        ..write(offset++, 0x5c)
        ..write(offset++, 0x72);
    default:
      if (code < 0x20) {
        offset = _writeUnicodeEscape(out, offset, code);
      } else if (_isUnpairedSurrogateAt(source, index)) {
        offset = _writeSurrogateEscape(out, offset, code);
      } else {
        out.write(offset++, code);
      }
  }
  return offset;
}

int _writeUnicodeEscape(WasmArray<WasmI16> out, int offset, int code) {
  out
    ..write(offset++, 0x5c)
    ..write(offset++, 0x75)
    ..write(offset++, 0x30)
    ..write(offset++, 0x30)
    ..write(offset++, _hexDigit((code >> 4) & 0xf))
    ..write(offset++, _hexDigit(code & 0xf));
  return offset;
}

int _writeUnicodeEscapeI8(WasmArray<WasmI8> out, int offset, int code) {
  out
    ..write(offset++, 0x5c)
    ..write(offset++, 0x75)
    ..write(offset++, 0x30)
    ..write(offset++, 0x30)
    ..write(offset++, _hexDigit((code >> 4) & 0xf))
    ..write(offset++, _hexDigit(code & 0xf));
  return offset;
}

int _hexDigit(int value) => value < 10 ? 0x30 + value : 0x57 + value;

/// Whether the code unit at [index] is a surrogate that does not form a
/// pair, i.e. a low surrogate or a high one not followed by a low one.
bool _isUnpairedSurrogateAt(WasmStringImplementation source, int index) {
  final c = source.codeUnitAtUnchecked(index);
  final isHigh = c >= 0xD800 && c <= 0xDBFF;
  final isLow = c >= 0xDC00 && c <= 0xDFFF;
  if (!isHigh && !isLow) return false;
  if (isLow) return true;
  final next = index + 1 < source.length ? source.codeUnitAtUnchecked(index + 1) : 0;
  return !(next >= 0xDC00 && next <= 0xDFFF);
}

/// Writes `\udXXX` (lowercase hex) for an unpaired surrogate.
int _writeSurrogateEscape(WasmArray<WasmI16> out, int offset, int code) {
  out
    ..write(offset++, 0x5c)
    ..write(offset++, 0x75)
    ..write(offset++, _hexDigit((code >> 12) & 0xf))
    ..write(offset++, _hexDigit((code >> 8) & 0xf))
    ..write(offset++, _hexDigit((code >> 4) & 0xf))
    ..write(offset++, _hexDigit(code & 0xf));
  return offset;
}
