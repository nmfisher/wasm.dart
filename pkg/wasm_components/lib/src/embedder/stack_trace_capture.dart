// ignore: import_internal_library
import 'dart:_wasm';

import 'constants.dart';
import 'libc.dart' as libc;
import 'string.dart';

/// The component `capture-utf16` function, as the core module sees it.
///
/// The component function returns a `string`, which canon-lowers to a
/// retptr call: the guest passes the capture capacity and the address of a
/// two-word return area, and the host fills it with the `(pointer, length)`
/// pair of a UTF-16 buffer it allocated in linear memory through the
/// module's own realloc.
@pragma('wasm:import', 'component.implicitImport_stackTraceCaptureUtf16')
external WasmVoid _captureTrace(
  WasmI32 capacityUnits,
  WasmI32 returnArea,
);

const _captureAlign = 4;

/// How many UTF-16 code units the guest asks the host to capture.
///
/// Stack traces are diagnostic output; this bounds the host's work without
/// losing the frames near the top of the stack. Hosts that render shorter
/// traces simply report a shorter length.
const _captureCapacity = 8192;

/// Captures the current stack trace from the host and materializes it as a
/// guest GC string, or `null` when the host has nothing to capture.
///
/// This is the guest half of `stackTraceGetCurrent`: the trace only exists
/// in the host (wasmtime walks its own frame table, Node formats its
/// `Error().stack`), so the host renders the trace into linear memory as
/// UTF-16 and this function reads it back as code units while the captured
/// frames are still on the stack.
///
/// When no host capture is linked (a component built without
/// `dart.stackTraceGetCurrent` being used), the transform replaces this
/// import with a stub writing a null pointer into the return area and the
/// SDK-visible `stackTraceGetCurrent` export falls back to the
/// `UnsupportedStackTrace` constant like before.
WasmStringImplementation? tryCaptureStackTrace() {
  final returnArea = libc.mallocAligned(
    const WasmI32(_captureAlign),
    const WasmI32(8),
  );
  _captureTrace(const WasmI32(_captureCapacity), returnArea);

  final area = returnArea.toIntUnsigned();
  final pointer = libc.memory.loadInt32(area, align: 2).toIntUnsigned();
  final length = libc.memory.loadInt32(area + 4, align: 2).toIntUnsigned();
  libc.dartFree(returnArea, const WasmI32(8), const WasmI32(_captureAlign));

  if (pointer == 0) {
    // Nothing captured: the stub writes a null pointer without allocating.
    return null;
  }

  final units = WasmArray<WasmI16>(length);
  for (var i = 0; i < length; i++) {
    units.write(
      i,
      libc.memory.loadInt16(pointer + 2 * i, align: 1).toIntUnsigned(),
    );
  }
  // The host allocated the trace through the module's own realloc, so the
  // bytes go back through the same allocator (a zero-size free is a no-op).
  libc.dartFree(
    WasmI32.fromInt(pointer),
    WasmI32.fromInt(2 * length),
    const WasmI32(2),
  );  return Utf16String.unsafeWrap(units);
}

/// Renders the string that `stackTraceGetCurrent` captured.
///
/// The SDK contract hands back the opaque externref from the capture and
/// passes it here from `_EmbedderStackTrace.toString`. Rendering it — not
/// capturing again — is what preserves the user's frames: by toString time
/// the stack has unwound, so a re-capture would show only the toString
/// machinery. Stubbed builds (no `component.implicitImport_stackTraceCaptureUtf16`
/// dependency) have `stackTraceGetCurrent` return the `UnsupportedStackTrace`
/// sentinel instead of a string; the `is` check recognizes it and falls back
/// to the SDK's unavailable message like before.
WasmStringImplementation stackTraceToStringImpl(WasmExternRef? trace) {
  final object = trace!.internalize().toObject();
  if (object is WasmStringImplementation) {
    return object;
  }
  return stackTracesAreUnavailableMessage;
}
