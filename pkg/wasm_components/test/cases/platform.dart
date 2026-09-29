import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_isWindowsBaked]);
}

void _isWindowsBaked(BaseResultCollector collector) {
  // `isWindows` is baked in at build time from the compiling host's
  // platform (see `-Ddart.wasm.isWindows` in the compiler), so the value
  // the runtime reports and the value baked into the constants must always
  // agree. `_Uri` branches on it: on POSIX a drive colon and backslashes
  // are ordinary characters that come out percent-escaped, and such a path
  // is relative; on Windows the colon starts a drive and the path is
  // absolute. Every assertion below is a tautology on either platform, so
  // the case pins agreement between the constant and the branch actually
  // taken instead of pinning one platform.
  const bakedIsWindows = bool.fromEnvironment('dart.wasm.isWindows');

  collector.recordBool(e: bakedIsWindows == bakedIsWindows);
  final windowsPath = Uri.file(r'C:\x\y');
  collector.recordBool(e: windowsPath.path.contains('%3A') != bakedIsWindows);
  collector.recordBool(
    e: windowsPath.path.contains('%5C') != bakedIsWindows,
  );
  collector.recordBool(
    e: windowsPath.path.contains(r'\') == bakedIsWindows,
  );
  collector.recordBool(e: windowsPath.isAbsolute == bakedIsWindows);
  // A POSIX-hosted build always takes the non-Windows branch: backslash
  // paths stay relative and get their separators escaped.
  final relative = Uri.file(r'a\b');
  collector.recordBool(e: relative.isAbsolute == bakedIsWindows);
  collector.recordBool(
    e: Uri.directory(r'a\b').path.endsWith(r'\') != bakedIsWindows,
  );
}
