// ignore: import_internal_library
import 'dart:_wasm';

import 'string.dart';

/// Encodes [source] as a JSON string literal (including the surrounding
/// quotes), as required by the SDK's internal JSON encoder for error strings:
///
/// - `"` becomes `\"`, `\` becomes `\\`
/// - `\b \t \n \f \r` use their short escapes
/// - other control characters become `\u00xx` with lowercase hex
/// - unpaired surrogates become lowercase \uXXXX escapes
/// - valid surrogate pairs and other non-ASCII code units are preserved
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
      extra += 5;
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
    final code = source.codeUnitAtUnchecked(i);
    offset = _isUnpairedSurrogateAt(source, i)
        ? _writeUnicodeEscape(out, offset, code)
        : _writeEscaped(out, offset, code);
  }
  out.write(offset, 0x22);
  return Utf16String.unsafeWrap(out);
}

/// JSON-escapes [code] into [out] at [offset]; returns the next offset.
int _writeEscaped(WasmArray<WasmI16> out, int offset, int code) {
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
    ..write(offset++, _hexDigit((code >> 12) & 0xf))
    ..write(offset++, _hexDigit((code >> 8) & 0xf))
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

/// Preserve both halves of a valid UTF-16 pair; escape only isolated halves.
bool _isUnpairedSurrogateAt(WasmStringImplementation source, int index) {
  final code = source.codeUnitAtUnchecked(index);
  if (code >= 0xd800 && code <= 0xdbff) {
    if (index + 1 == source.length) return true;
    final next = source.codeUnitAtUnchecked(index + 1);
    return next < 0xdc00 || next > 0xdfff;
  }
  if (code >= 0xdc00 && code <= 0xdfff) {
    if (index == 0) return true;
    final previous = source.codeUnitAtUnchecked(index - 1);
    return previous < 0xd800 || previous > 0xdbff;
  }
  return false;
}
