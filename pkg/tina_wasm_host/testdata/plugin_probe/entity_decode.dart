/// Pure stateless HTML-entity decoding, shared by the entity-decode plugin
/// fixture and the native/wasm parity tests. No package imports: this file
/// compiles both as ordinary Dart (native parity side) and inside the
/// whole-program plugin component (wasm side).
String decodeEntities(String input) {
  if (!input.contains('&')) {
    return input;
  }
  final out = StringBuffer();
  var i = 0;
  while (i < input.length) {
    final ch = input[i];
    if (ch != '&') {
      out.write(ch);
      i += 1;
      continue;
    }
    final semicolon = input.indexOf(';', i + 1);
    // No terminator (or too far away): the '&' is literal text.
    if (semicolon < 0 || semicolon - i > 12) {
      out.write(ch);
      i += 1;
      continue;
    }
    final body = input.substring(i + 1, semicolon);
    final decoded = _decodeBody(body);
    if (decoded == null) {
      out.write(ch);
      i += 1;
    } else {
      out.write(decoded);
      i = semicolon + 1;
    }
  }
  return out.toString();
}

String? _decodeBody(String body) {
  const named = {
    'amp': '&',
    'lt': '<',
    'gt': '>',
    'quot': '"',
    'apos': "'",
    'nbsp': '\u00a0',
  };
  if (named.containsKey(body)) {
    return named[body];
  }
  if (body.startsWith('#')) {
    final digits = body.substring(1);
    final code = digits.isNotEmpty && (digits[0] == 'x' || digits[0] == 'X')
        ? int.tryParse(digits.substring(1), radix: 16)
        : int.tryParse(digits);
    if (code == null || code < 0 || code > 0x10ffff) {
      return null;
    }
    return String.fromCharCode(code);
  }
  return null;
}
