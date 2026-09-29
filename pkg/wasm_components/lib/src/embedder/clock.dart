// ignore: import_internal_library
import 'dart:_wasm';

import 'constants.dart';
import 'libc.dart' as libc;
import 'string.dart';

/// Writes a struct storing seconds and nanoseconds into the given address.
///
/// This import is replaced with a stub if [DateTime.now] isn't used to avoid
/// the import dependency.
@pragma('wasm:import', 'dart.wasi_now')
external WasmVoid wasiNowPtr(WasmI32 ptr);

// The `dart.wasi_iana_id` import was removed: wasmtime implements no
// `wasi:clocks/timezone` interface (see [wasiIanaId]), so there is no host
// left to call, and the component transform keeps handling the unused import
// only for modules still importing it.

@pragma('wasm:import', 'dart.wasi_monotonic_now')
external WasmI64 wasiMonotonicNow();

@pragma('wasm:import', 'dart.wasi_monotonic_getResolution')
external WasmI64 wasiMonotonicGetResolution();

@pragma('wasm:import', 'dart.wasi_monotonic_waitFor')
external WasmI32 wasiMonotonicWaitFor(WasmI64 durationInNanos);

int wasiTimestampInMicroseconds() {
  final instantPtr = libc.mallocAligned(const WasmI32(8), const WasmI32(16));
  wasiNowPtr(instantPtr);

  final address = instantPtr.toIntUnsigned();
  final seconds = libc.memory.loadInt64(address, align: 3).toInt();
  final plusNanos = libc.memory
      .loadInt32(address, align: 2, offset: 8)
      .toIntUnsigned();
  final plusMicros = plusNanos ~/ 1000;
  libc.dartFree(instantPtr, const WasmI32(16), const WasmI32(8));

  return (seconds * 1_000_000) + plusMicros;
}

WasmStringImplementation wasiIanaId() {
  // The name has to pair with `timeZoneOffsetInSecondsForClampedSeconds`,
  // which reports a constant zero offset: without a tz database in the guest
  // there is no way to name a zone whose offset we do not know, and the
  // `wasi:clocks/timezone.iana-id` host call this import was modeled on is
  // not implemented by wasmtime (its p3 clocks linker only serves the
  // wall/monotonic clock interfaces). So the id claims UTC, matching the
  // offset that is always reported with it. Implementing `timezone` in
  // wasmtime-wasi's host clocks would lift this; see the README's known
  // limitations.
  return unknownTimezone;
}

int _tickFrequency = 0;
int _tickFactor = 1;

void _initializeFrequency() {
  if (_tickFrequency == 0) {
    final durationNanos = wasiMonotonicGetResolution().toInt();
    if (durationNanos <= 1000) {
      // Native timer ticks in microseconds or faster, use MHz as a base unit.
      _tickFrequency = 1_000_000;
      _tickFactor = 1000;
    } else {
      // Native timer ticks slower than 1 MHz, report as ticks in 1 kHz.
      _tickFrequency = 1000;
      _tickFactor = 1_000_000;
    }
  }
}

// Dart only supports us returning 1 kHz or 1 MHz here. Use a frequency
// appropriate for the runtime.
int get dartStopwatchTickFrequency {
  _initializeFrequency();
  return _tickFrequency;
}

int get dartMonotonicTicks {
  _initializeFrequency();

  // Note: wasiMonotonicNow is always a number of nanoseconds, Dart wants this
  // represented in units of ticks.
  return wasiMonotonicNow().toInt() ~/ _tickFactor;
}
