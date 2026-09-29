// Stub implementations for the `dart.weak*` family of imports:
// WeakReference, Expando and Finalizer all need GC-observing host support
// (or wasmgc weak-ref proposals) that does not exist yet, so we can only
// provide objects that keep the SDK working while dropping the semantics.
library;

// ignore: import_internal_library
import 'dart:_wasm';

import 'libc.dart' as libc;
import 'string.dart';
import 'utils.dart';

/// Writes a string's UTF-16 code units into linear memory for a canonical
/// `(pointer, length)` crossing.
///
/// Canonical `string` imports pass the characters through the module's own
/// linear memory; the units must stay alive until the host has copied them,
/// so the allocation is freed only after the call returned (see
/// [_freeUnits]).
final class _EncodedUnits {
  final WasmI32 ptr;
  final WasmI32 packedLength;

  _EncodedUnits(this.ptr, this.packedLength);
}

_EncodedUnits _writeUnits(WasmStringImplementation source) {
  final length = source.length;
  final ptr = libc.mallocAligned(
    const WasmI32(2),
    WasmI32.fromInt(2 * length),
  );
  final address = ptr.toIntUnsigned();
  for (var i = 0; i < length; i++) {
    libc.memory.storeInt16(
      address + 2 * i,
      WasmI32.uint16FromInt(source.codeUnitAtUnchecked(i)),
      align: 1,
      );
  }
  return _EncodedUnits(ptr, WasmI32.fromInt(length));
}

void _freeUnits(_EncodedUnits units) {
  libc.dartFree(
    units.ptr,
    WasmI32.fromInt(2 * units.packedLength.toIntSigned()),
    const WasmI32(2),
  );
}

/// One entry of the stub expando: an object target (compared by identity)
/// and the value associated with it.
final class _ExpandoEntry {
  final WasmAnyRef target;
  final WasmAnyRef? value;
  _ExpandoEntry(this.target, this.value);
}

final class _EmbedderExpando {
  final List<_ExpandoEntry> entries = <_ExpandoEntry>[];
}

final class _EmbedderWeakRef {
  final WasmAnyRef? target;
  _EmbedderWeakRef(this.target);
}

final class _EmbedderFinalizer {
  // Registered objects are intentionally dropped: without a GC there is
  // nothing to finalize, so neither the callbacks nor the detach tokens are
  // retained. `finalizerDetach` stays a no-op that always succeeds.
  const _EmbedderFinalizer();
}

/// Compares two anyrefs by identity: `WasmAnyRef` has no `==`, so box the
/// references and use Dart object identity.
bool _sameRef(WasmAnyRef? a, WasmAnyRef? b) =>
    identical(a?.toObject(), b?.toObject());

/// (`dart.weakRefCreate`) Holds [originalValue] strongly.
@pragma('wasm:export')
WasmExternRef weakRefCreate(WasmAnyRef originalValue) {
  return WasmAnyRef.fromObject(_EmbedderWeakRef(originalValue)).externalize();
}

/// (`dart.weakRefGet`) Returns the held value — always non-null for a ref
/// this module handed out.
@pragma('wasm:export')
WasmAnyRef? weakRefGet(WasmExternRef? weakReference) {
  final ref = weakReference!.internalize().toObject() as _EmbedderWeakRef;
  return ref.target;
}

/// (`dart.expandoCreate`)
@pragma('wasm:export')
WasmExternRef expandoCreate() {
  return WasmAnyRef.fromObject(_EmbedderExpando()).externalize();
}

/// (`dart.expandoGet`) The SDK passes the target's identity hash, which the
/// real host uses to find its own entry; here the target identity alone is
/// enough because we keep the entries ourselves.
@pragma('wasm:export')
WasmAnyRef? expandoGet(
  WasmExternRef? expando,
  WasmAnyRef target,
  WasmI64 targetIdentityHashCode,
) {
  final embedderExpando = expando!.internalize().toObject() as _EmbedderExpando;
  for (final entry in embedderExpando.entries) {
    if (_sameRef(entry.target, target)) return entry.value;
  }
  return null;
}

/// (`dart.expandoSet`)
@pragma('wasm:export')
WasmVoid expandoSet(
  WasmExternRef? expando,
  WasmAnyRef target,
  WasmI64 targetIdentityHashCode,
  WasmAnyRef? value,
) {
  final embedderExpando = expando!.internalize().toObject() as _EmbedderExpando;
  for (var i = 0; i < embedderExpando.entries.length; i++) {
    if (_sameRef(embedderExpando.entries[i].target, target)) {
      embedderExpando.entries[i] = _ExpandoEntry(target, value);
      return WasmVoid();
    }
  }
  embedderExpando.entries.add(_ExpandoEntry(target, value));
  return WasmVoid();
}

/// (`dart.finalizerCreate`) The callback and first parameter are dropped:
/// with no GC nothing ever becomes unreachable, so no callback could run.
@pragma('wasm:export')
WasmExternRef finalizerCreate(
  WasmFunction<WasmVoid Function(WasmAnyRef, WasmAnyRef?)> callback,
  WasmAnyRef firstParameter,
) {
  return WasmAnyRef.fromObject(const _EmbedderFinalizer()).externalize();
}

/// (`dart.finalizerAttach`)
@pragma('wasm:export')
WasmVoid finalizerAttach(
  WasmExternRef? finalizer,
  WasmAnyRef object,
  WasmAnyRef? token,
  WasmAnyRef? detachToken,
) {
  final _EmbedderFinalizer _ =
      finalizer!.internalize().toObject() as _EmbedderFinalizer;
  return WasmVoid();
}

/// (`dart.finalizerDetach`)
@pragma('wasm:export')
WasmVoid finalizerDetach(WasmExternRef? finalizer, WasmAnyRef detachToken) {
  final _EmbedderFinalizer _ =
      finalizer!.internalize().toObject() as _EmbedderFinalizer;
  return WasmVoid();
}

/// (`dart.baseUri`) The file URI of the directory that holds the compiled
/// entry point, baked in by the compiler as the `dart.wasm.baseUri` define
/// (there is no process working directory to read at runtime: the component
/// runs on a host that never gave it a filesystem identity).
@pragma('wasm:export')
WasmExternRef? baseUri() {
  final units = WasmArray<WasmI16>(_baseUri.length);
  for (var i = 0; i < _baseUri.length; i++) {
    units.write(i, _baseUri.codeUnitAt(i));
  }
  return Utf16String.unsafeWrap(units).externalize();
}

/// (`dart.isWindows`) The platform the build ran on, baked in by the
/// compiler as the `dart.wasm.isWindows` define: the component carries no
/// host-side OS identity it could ask about at runtime.
@pragma('wasm:export')
WasmI32 isWindows() {
  return WasmI32.fromBool(_isWindows);
}

/// The file URI of the entry-point directory, written by the compiler.
const String _baseUri = String.fromEnvironment('dart.wasm.baseUri');

/// Whether the build ran on Windows, written by the compiler.
const bool _isWindows = bool.fromEnvironment('dart.wasm.isWindows');

/// (`dart.timelineStreamEnabled`) Reports that the timeline stream is
/// listened to: events flow to a real sink, a host import whose events the
/// wasm hosts write as one NDJSON line each onto their stderr.
@pragma('wasm:export')
WasmI32 timelineStreamEnabled() {
  return WasmI32.fromBool(true);
}

/// The host sink for timeline events, as the core module sees it.
///
/// The SDK import takes the strings as externrefs (`dart.reportTaskEvent`
/// is a module import and never crosses the component boundary), so this
/// component-level sink takes them canonically: flat u8 event type and
/// `(pointer, length)` UTF-16 pairs in the module's linear memory. The
/// host-side sink writes one NDJSON line per event to its stderr - wasm
/// hosts have no other channel that never mixes with the goldens.
@pragma('wasm:import', 'component.implicitImport_timelineReportTaskEvent')
external WasmI32 _timelineReportTaskEvent(
  WasmI32 type,
  WasmI32 taskId,
  WasmI32 flowId,
  WasmI32 namePtr,
  WasmI32 nameLengthUnits,
  WasmI32 argsPtr,
  WasmI32 argsLengthUnits,
);

/// (`dart.reportTaskEvent`) Forwards the event to the host sink, converting
/// the SDK's externref strings into the canonical `(pointer, length)` pairs
/// the component interface carries.
///
/// The host returns whether the event was recorded; the embedder passes it
/// through unchanged (the VM's `_reportTaskEvent` also just records).
@pragma('wasm:export')
WasmI32 reportTaskEvent(
  WasmI32 taskId,
  WasmI32 flowId,
  WasmI32 type,
  WasmExternRef? name,
  WasmExternRef? argumentsAsJson,
) {
  final nameString = WasmStringImplementation.fromExtern(name);
  final argumentsString = WasmStringImplementation.fromExtern(argumentsAsJson);
  final namePtr = _writeUnits(nameString);
  final argsPtr = _writeUnits(argumentsString);
  final recorded = _timelineReportTaskEvent(
    type,
    taskId,
    flowId,
    namePtr.ptr,
    namePtr.packedLength,
    argsPtr.ptr,
    argsPtr.packedLength,
  );
  _freeUnits(namePtr);
  _freeUnits(argsPtr);
  return recorded;
}

/// (`dart.inspect`) No debugger can be attached in this embedder; the
/// reference is dropped and the value echoed back by the SDK patch.
@pragma('wasm:export')
WasmVoid inspect(WasmAnyRef? object) {
  return WasmVoid();
}
