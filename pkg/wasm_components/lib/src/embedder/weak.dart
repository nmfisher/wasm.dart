// Stub implementations for the `dart.weak*` family of imports:
// WeakReference, Expando and Finalizer all need GC-observing host support
// (or wasmgc weak-ref proposals) that does not exist yet, so we can only
// provide objects that keep the SDK working while dropping the semantics.
library;

// ignore: import_internal_library
import 'dart:_wasm';

import 'string.dart';
import 'utils.dart';

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

/// (`dart.baseUri`) There is no process URL in this embedder; hand back the
/// filesystem root so `Uri.base` resolves instead of throwing.
@pragma('wasm:export')
WasmExternRef? baseUri() {
  return WasmStringImplementation.fromDartString('file:///').externalize();
}


/// (`dart.isWindows`) This embedder never runs on Windows.
@pragma('wasm:export')
WasmI32 isWindows() {
  return WasmI32.fromBool(false);
}
