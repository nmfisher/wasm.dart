/// Float32x4-heavy benchmark kernel (stable-SDK validation spike).
/// Checksum-only stdout (byte-stable, golden-checked); this is the SIMD
/// case that exercises v128 opcodes through the whole wasm_tools pipeline.
library;

import 'dart:typed_data';

import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_benchKernel]);
}

void _benchKernel(BaseResultCollector collector) {
  const rounds = 6;
  const iters = 200000;
  final a = Float32x4(1.5, 2.5, 3.5, 4.5);
  final b = Float32x4(0.5, 1.0, 1.5, 2.0);
  var acc = Float32x4.zero();
  var guard = 0.0;
  for (var r = 0; r < rounds; r++) {
    var v = a;
    for (var i = 0; i < iters; i++) {
      final m = v * b;
      v = m + acc;
      acc = v + acc;
      v = v.scale(1.0000001);
    }
    final sum = v.x + v.y + v.z + v.w + acc.x;
    guard += sum;
  }
  guard = guard.isInfinite ? 1.0 : guard;
  print('ok ${guard.toStringAsFixed(6)}');
  collector.recordBool(e: true);
}
