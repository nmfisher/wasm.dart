// ignore: import_internal_library
import 'dart:_wasm';
import 'dart:async';
import 'dart:typed_data';

import '../runtime/async/future.dart';
import '../runtime/async/stream.dart';
import '../runtime/result.dart';
import 'libc.dart';
import 'string.dart';

/// Prints [string] to the host's stdout.
///
/// This is called by the `print` embedder export, which is only reachable in
/// compiled programs when the component compiler has rewritten it into a real
/// `wasi:cli/stdout` dependency. In raw module mode without that rewrite, the
/// original no-op export is used instead (see the transform in pkg:wasm_tools).
void printImpl(WasmStringImplementation string) {
  // dart:core's `print` is line oriented, but this hook receives the line
  // without its terminator, so re-add the newline here.
  final message = string.concat(_newline);

  // Encode as UTF-8, reserving an extra byte per code unit for surrogates
  // expanding to two bytes.
  final length = message.length;
  final bytes = Uint8List(length * 3);
  var count = 0;
  for (var i = 0; i < length; i++) {
    final unit = message.codeUnitAtUnchecked(i);
    if (unit < 0x80) {
      bytes[count++] = unit;
    } else if (unit < 0x800) {
      bytes[count++] = 0xC0 | (unit >> 6);
      bytes[count++] = 0x80 | (unit & 0x3F);
    } else {
      bytes[count++] = 0xE0 | (unit >> 12);
      bytes[count++] = 0x80 | ((unit >> 6) & 0x3F);
      bytes[count++] = 0x80 | (unit & 0x3F);
    }
  }

  final readable = newReadableStream(_U8StreamVtable(), Stream.value(bytes));

  // Hand the readable end to the host via wasi:cli/stdout and wait for the
  // write to complete, so that print statements appear before `run` returns.
  final future = _writeViaStream(readable.toWasmI32());
  unawaited(readFuture(_WriteResultVtable(), future.toIntUnsigned()));
}

const _newline = Latin1String.unsafeWrap(WasmArray.literal([10]));

@pragma('wasm:import', 'component.implicitImport_stdoutWriteViaStream')
external WasmI32 _writeViaStream(WasmI32 stream);

final class _U8StreamVtable implements StreamVtable<Uint8List> {
  const _U8StreamVtable();

  @override
  int get elementSize => 1;

  @override
  int allocateBuffer(int size) {
    return mallocAligned(const WasmI32(1), WasmI32.fromInt(size))
        .toIntUnsigned();
  }

  @override
  void freeBuffer(int address, int totalSize, int start, int end) {
    dartFree(
      WasmI32.fromInt(address),
      WasmI32.fromInt(totalSize),
      const WasmI32(1),
    );
  }

  @override
  void writeToBuffer(int address, Uint8List elements) {
    for (var i = 0; i < elements.length; i++) {
      memory.storeInt8(
        WasmI32.fromInt(address + i).toIntUnsigned(),
        WasmI32.uint8FromInt(elements[i]),
      );
    }
  }

  @override
  Uint8List readFromBuffer(int address, int count) {
    final result = Uint8List(count);
    for (var i = 0; i < count; i++) {
      result[i] = memory
          .loadUint8(WasmI32.fromInt(address + i).toIntUnsigned())
          .toIntUnsigned();
    }
    return result;
  }

  @override
  int newStream() => _streamNew().toInt();

  @override
  int write(int stream, int ptr, int n) {
    return _streamWrite(
      WasmI32.fromInt(stream),
      WasmI32.fromInt(ptr),
      WasmI32.fromInt(n),
    ).toIntUnsigned();
  }

  @override
  int read(int stream, int ptr, int n) {
    return _streamRead(
      WasmI32.fromInt(stream),
      WasmI32.fromInt(ptr),
      WasmI32.fromInt(n),
    ).toIntUnsigned();
  }

  @override
  void dropWritable(int stream) {
    _streamDropWritable(WasmI32.fromInt(stream));
  }

  @override
  void dropReadable(int stream) {
    _streamDropReadable(WasmI32.fromInt(stream));
  }
}

@pragma('wasm:import', 'component.implicitImport_stdoutStreamNew')
external WasmI64 _streamNew();

@pragma('wasm:import', 'component.implicitImport_stdoutStreamWrite')
external WasmI32 _streamWrite(WasmI32 stream, WasmI32 ptr, WasmI32 count);

@pragma('wasm:import', 'component.implicitImport_stdoutStreamRead')
external WasmI32 _streamRead(WasmI32 stream, WasmI32 ptr, WasmI32 count);

@pragma('wasm:import', 'component.implicitImport_stdoutStreamDropReadable')
external WasmVoid _streamDropReadable(WasmI32 stream);

@pragma('wasm:import', 'component.implicitImport_stdoutStreamDropWritable')
external WasmVoid _streamDropWritable(WasmI32 stream);

final class _WriteResultVtable
    implements FutureVtable<Result<void, StdoutErrorCode>> {
  const _WriteResultVtable();

  @override
  int newFuture() => _futureNew().toInt();

  @override
  int read(int future, int buffer) {
    return _futureRead(
      WasmI32.fromInt(future),
      WasmI32.fromInt(buffer),
    ).toIntUnsigned();
  }

  @override
  int write(int future, int buffer) {
    return _futureWrite(
      WasmI32.fromInt(future),
      WasmI32.fromInt(buffer),
    ).toIntUnsigned();
  }

  @override
  void dropRead(int future) {
    _futureDropReadable(WasmI32.fromInt(future));
  }

  @override
  void dropWrite(int future) {
    _futureDropWritable(WasmI32.fromInt(future));
  }

  @override
  int allocateBuffer() {
    // A `result<_, error-code>` with a 3-case enum error: one tag byte plus
    // one payload byte.
    return mallocAligned(const WasmI32(1), const WasmI32(2)).toIntUnsigned();
  }

  @override
  void freeBuffer(int address, {required bool containsValue}) {
    dartFree(WasmI32.fromInt(address), const WasmI32(2), const WasmI32(1));
  }

  @override
  Result<void, StdoutErrorCode> load(int address) {
    final unsigned = WasmI32.fromInt(address).toIntUnsigned();
    if (!memory.loadUint8(unsigned).toBool()) {
      return const OkResult(null);
    }

    final index = memory.loadUint8(unsigned, offset: 1).toIntUnsigned();
    return ErrorResult(StdoutErrorCode.values[index]);
  }

  @override
  void store(int address, Result<void, StdoutErrorCode> value) {
    throw UnimplementedError('stdout results are only ever read');
  }
}

/// The `error-code` type of `wasi:cli/types@0.3.0`.
enum StdoutErrorCode { io, illegalByteSequence, pipe }

@pragma('wasm:import', 'component.implicitImport_stdoutFutureNew')
external WasmI64 _futureNew();

@pragma('wasm:import', 'component.implicitImport_stdoutFutureWrite')
external WasmI32 _futureWrite(WasmI32 future, WasmI32 buffer);

@pragma('wasm:import', 'component.implicitImport_stdoutFutureRead')
external WasmI32 _futureRead(WasmI32 future, WasmI32 buffer);

@pragma('wasm:import', 'component.implicitImport_stdoutFutureDropReadable')
external WasmVoid _futureDropReadable(WasmI32 future);

@pragma('wasm:import', 'component.implicitImport_stdoutFutureDropWritable')
external WasmVoid _futureDropWritable(WasmI32 future);
