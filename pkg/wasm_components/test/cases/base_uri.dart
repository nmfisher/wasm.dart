import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_baseUri]);
}

void _baseUri(BaseResultCollector collector) {
  // `Uri.base` is baked in at build time as the file URI of the directory
  // holding the compiled entry point (see `-Ddart.wasm.baseUri` in the
  // compiler). The VM oracle reads the process working directory instead,
  // so this case only records location-independent facts: a `file:` URL
  // that is absolute, names a directory (trailing `/`, non-empty path
  // segments), and resolves relative references against itself.
  final base = Uri.base;
  collector.recordBool(e: base.scheme == 'file');
  collector.recordBool(e: base.isAbsolute);
  collector.recordBool(e: base.path.endsWith('/'));
  collector.recordBool(e: base.pathSegments.isNotEmpty);
  collector.recordBool(e: base.hasEmptyPath == false);
  final resolved = base.resolve('main.dart');
  collector.recordBool(e: resolved.path.endsWith('/main.dart'));
  collector.recordBool(e: resolved.scheme == 'file');
}
