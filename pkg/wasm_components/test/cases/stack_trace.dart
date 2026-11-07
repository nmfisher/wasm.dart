import 'package:test_runner/test_runner.dart';

@pragma('wasm:never-inline')
StackTrace grabTrace() {
  return StackTrace.current;
}

void main() {
  defineTests(const [_stackTrace]);
}

void _stackTrace(BaseResultCollector collector) {
  final trace = grabTrace().toString();
  // Line-based details differ between runtimes (the VM prints frame counts
  // and file URIs, a wasm embedder prints wasm frames, and the frame *names*
  // come from each host's renderer), so only assert the structural contract:
  // non-empty and multiple lines, from the capture that `grabTrace` itself
  // triggered.
  final lines = trace.split('\n').where((l) => l.trim().isNotEmpty).toList();
  collector.recordBool(e: trace.isNotEmpty);
  collector.recordBool(e: lines.length >= 2);
  collector.recordBool(e: trace.contains('_stackTrace') || trace.contains('main'));
}
