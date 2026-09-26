// Runs the test cases under test/cases as WebAssembly modules.
//
// The full pipeline (component_test.dart) needs a Rust toolchain to build the
// native test runner and the component linker. This script is a
// self-contained replacement used in environments without Rust:
//
//   1. Every case is compiled with the same dart2wasm flags the real compiler
//      uses (see `compiler.dart` in package:wasm_tools).
//   2. The module is instantiated in Node. The `dart.*` imports are linked to
//      the `wasm:export` functions of the same module, which is exactly the
//      linking the real component compiler performs. There are no JavaScript
//      fallbacks for `dart.*` imports: an import without a matching export
//      fails the test instead of silently passing.
//   3. The host side implements only what a host genuinely has to provide:
//      the linear memory with an allocator (`libc.*`), the component context
//      slot and the result collector (`component._import*`).
//
// The output of a run must match the golden file next to the case. Golden
// files are generated with `dart run tool/generate_test_goldens.dart`, which
// executes the cases on the Dart VM. The wasm run and the VM run therefore
// must agree on the recorded values.

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const toolDir = path.dirname(fileURLToPath(import.meta.url));
const packageDir = path.resolve(toolDir, '..');
const workspaceDir = path.resolve(packageDir, '..', '..');
const casesDir = path.join(packageDir, 'test', 'cases');

const dartSdk = process.env.DART_SDK ?? '/opt/dart-sdk';
const dartAotRuntime = path.join(dartSdk, 'bin', 'dartaotruntime');
const dart2wasmSnapshot = path.join(
  dartSdk,
  'bin',
  'snapshots',
  'dart2wasm_product.snapshot',
);
const librariesSpec = path.join(dartSdk, 'lib', 'libraries.json');
const packageConfig = path.join(
  workspaceDir,
  '.dart_tool',
  'package_config.json',
);

function compile(dartFile, outWasm) {
  execFileSync(
    dartAotRuntime,
    [
      dart2wasmSnapshot,
      '--libraries-spec',
      librariesSpec,
      '--packages',
      packageConfig,
      '--standalone',
      '--enable-experimental-wasm-interop',
      '--no-minify',
      '--no-strip-wasm',
      '-O0',
      dartFile,
      outWasm,
    ],
    { stdio: ['ignore', 'pipe', 'pipe'] },
  );
}

function readU32(bytes, pos) {
  let result = 0;
  let shift = 0;
  for (;;) {
    const byte = bytes[pos++];
    result |= (byte & 0x7f) << shift;
    shift += 7;
    if (!(byte & 0x80)) return [result >>> 0, pos];
  }
}

function writeU32(value) {
  const out = [];
  do {
    let byte = value & 0x7f;
    value >>>= 7;
    if (value) byte |= 0x80;
    out.push(byte);
  } while (value);
  return Uint8Array.from(out);
}

/// dart2wasm emits `main` as a plain function that the runtime is expected to
/// call through `$invokeMain` (which needs a wasm argv array that JavaScript
/// cannot build). The module keeps a `name` section (we compile with
/// `--no-strip-wasm`), so we can look up the function index of `main` and
/// re-export it. The harness then calls it after instantiation, which is
/// what the component runtime would do.
function exportFunction(bytes, functionIndex, exportName) {
  let pos = 8;
  const out = [bytes.subarray(0, 8)];
  while (pos < bytes.length) {
    const sectionStart = pos;
    const id = bytes[pos++];
    const [size, afterSize] = readU32(bytes, pos);
    const sectionEnd = afterSize + size;
    if (id === 7) {
      const body = bytes.subarray(afterSize, sectionEnd);
      const [count, afterCount] = readU32(body, 0);
      const nameBytes = Buffer.from(exportName, 'utf8');
      const addition = Buffer.concat([
        writeU32(nameBytes.length),
        nameBytes,
        Uint8Array.from([0]),
        writeU32(functionIndex),
      ]);
      const newBody = Buffer.concat([
        Buffer.from(writeU32(count + 1)),
        Buffer.from(body.subarray(afterCount)),
        addition,
      ]);
      out.push(Uint8Array.from([7]), Buffer.from(writeU32(newBody.length)), newBody);
    } else {
      out.push(bytes.subarray(sectionStart, sectionEnd));
    }
    pos = sectionEnd;
  }
  return Buffer.concat(out);
}

/// Returns a map from function index to name from the module's name section.
function functionNames(bytes) {
  let pos = 8;
  while (pos < bytes.length) {
    const id = bytes[pos++];
    const [size, afterSize] = readU32(bytes, pos);
    if (id === 0) {
      const [len, namePos] = readU32(bytes, afterSize);
      const customName = bytes.subarray(namePos, namePos + len).toString('utf8');
      if (customName === 'name') {
        const body = bytes.subarray(namePos + len, afterSize + size);
        let p = 0;
        const names = {};
        while (p < body.length) {
          const sub = body[p++];
          const [subSize, next] = readU32(body, p);
          if (sub === 1) {
            // The "function names" subsection.
            let q = readU32(body, next)[1];
            const count = readU32(body, next)[0];
            for (let i = 0; i < count; i++) {
              let index;
              [index, q] = readU32(body, q);
              let length;
              [length, q] = readU32(body, q);
              names[index] = body.subarray(q, q + length).toString('utf8');
              q += length;
            }
          }
          p = next + subSize;
        }
        return names;
      }
    }
    pos = afterSize + size;
  }
  return {};
}

// The SDK declares the imported memory with a minimum of 17 pages. Static
// data of the compiled module lives at the start of that memory. Allocations
// start after a generous margin, growing the memory as needed.
const allocatorStart = 8 * 1024 * 1024;
const initialPages = Math.ceil(allocatorStart / 65536) + 17;

function makeLibc() {
  const memory = new WebAssembly.Memory({ initial: initialPages });
  let top = allocatorStart;

  function realloc(oldPtr, oldLen, alignLog2, newLen) {
    const alignment = 1 << (alignLog2 & 31);
    top = Math.ceil(top / alignment) * alignment;
    const required = top + (newLen >>> 0);
    const current = memory.buffer.byteLength;
    if (required > current) {
      const pages = Math.ceil((required - current) / 65536);
      memory.grow(pages);
    }
    const ptr = top;
    top = required;
    return ptr;
  }

  function free(_ptr, _sizeInBytes, _alignment) {
    // The bump allocator never reuses memory. Fine for tests.
  }

  return { memory, realloc, free };
}

function readUtf16(memory, ptr, packedLength) {
  const length = packedLength >>> 0;
  const bytes = new Uint16Array(memory.buffer, ptr >>> 0, length);
  let result = '';
  for (let i = 0; i < length; i++) {
    result += String.fromCharCode(bytes[i]);
  }
  return result;
}

function makeCollector() {
  const lines = [];

  function recordDouble(e) {
    // Matches how the Dart VM serializes doubles in JSON: whole numbers keep
    // a trailing `.0`.
    const value = Number.isInteger(e) ? `${e.toFixed(1)}` : `${e}`;
    lines.push(`{"type":"double","value":${value}}`);
  }

  function recordInt(e) {
    lines.push(`{"type":"int","value":${e}}`);
  }

  function recordBool(e) {
    lines.push(`{"type":"bool","value":${e !== 0}}`);
  }

  return { lines, recordDouble, recordInt, recordBool };
}

// ---------------------------------------------------------------------------
// A miniature component-model runtime.
//
// dart2wasm lowers `print` into a `wasi:cli/stdout` `write-via-stream`, and
// the guest runs every test case inside a task (see `spawnTask` in
// run_components.dart). Hosting that needs the same primitives a full
// component runtime provides:
//
//   * a handle table shared by the guest and this host,
//   * streams and futures speaking the copy protocol of the Canonical ABI
//     (`End.copy` and the notify closures below follow the pseudocode in the
//     "Stream and Future State" section of the spec),
//   * waitable sets, and
//   * an event pump that delivers pending events to the guest through its
//     exported `callback` function until the task is quiescent.
//
// The host owns the readable end of a printed stream once the guest hands it
// over through `write-via-stream`. It drains that stream into the test output
// and then completes the returned future with an `ok` result, which is what a
// wasi:cli host does for a successful write.

// `canon {stream,future}.{read,write}` return this when the copy cannot make
// progress yet and the event will be delivered through the event loop instead.
const BLOCKED = 0xffff_ffff;

const COPY_COMPLETED = 0;
const COPY_DROPPED = 1;
const COPY_CANCELLED = 2;

// See `EventCode` in the Canonical ABI (and waitable.dart).
const EVENT_SUBTASK = 1;
const EVENT_STREAM_READ = 2;
const EVENT_STREAM_WRITE = 3;
const EVENT_FUTURE_READ = 4;
const EVENT_FUTURE_WRITE = 5;

// The low 4 bits of the value returned by the exported `callback` function.
const CALLBACK_EXIT = 0;
const CALLBACK_YIELD = 1;
const CALLBACK_WAIT = 2;

// Element types of the streams and futures used here. Only the element size
// matters to the host: the guest decides how elements are laid out.
const U8 = { size: 1 };
// `result<_, error-code>` of wasi:cli/types: one tag byte plus one payload
// byte (see `_WriteResultVtable` in tmp_print.dart).
const STDOUT_RESULT = { size: 2 };
const VOID = { size: 0 };

class Waitable {
  pendingEvent = null;
  wset = null;

  hasPendingEvent() {
    return this.pendingEvent !== null;
  }

  getPendingEvent() {
    const event = this.pendingEvent;
    this.pendingEvent = null;
    return event();
  }

  join(wset) {
    if (this.wset !== null) {
      this.wset.elems.splice(this.wset.elems.indexOf(this), 1);
    }
    this.wset = wset;
    if (wset !== null) {
      wset.elems.push(this);
    }
  }
}

class WaitableSet {
  elems = [];

  // The spec allows any scheduling; keep the creation order so runs are
  // reproducible.
  findReady() {
    for (const waitable of this.elems) {
      if (waitable.hasPendingEvent()) return waitable;
    }
    return null;
  }
}

// A buffer over guest memory. `progress` counts transferred elements; `read`
// is only called on the writing side's buffer and `write` only on the reading
// side's buffer, exactly like the `ReadableBuffer`/`WritableBuffer` pair of
// the spec.
class GuestBuffer {
  constructor(memory, ptr, length, elementSize) {
    this.memory = memory;
    this.ptr = ptr >>> 0;
    this.length = length >>> 0;
    this.elementSize = elementSize;
    this.progress = 0;
    const bytes = this.length * this.elementSize;
    if (this.ptr + bytes > memory.buffer.byteLength) {
      throw new Error(
        `buffer [${this.ptr}, ${this.ptr + bytes}) is outside memory`,
      );
    }
  }

  remain() {
    return this.length - this.progress;
  }

  read(n) {
    const start = this.ptr + this.progress * this.elementSize;
    this.progress += n;
    return new Uint8Array(
      this.memory.buffer,
      start,
      n * this.elementSize,
    ).slice();
  }

  write(bytes) {
    const start = this.ptr + this.progress * this.elementSize;
    this.progress += this.elementSize === 0 ? 1 : bytes.length / this.elementSize;
    new Uint8Array(this.memory.buffer, start, bytes.length).set(bytes);
  }
}

// The host side of a copy: either a write destination with spare capacity
// (when the host reads a stream) or a read source holding a value (when the
// host completes a future).
class HostBuffer {
  constructor(capacity, elementSize, bytes = null) {
    this.length = capacity;
    this.elementSize = elementSize;
    this.progress = 0;
    this.bytes = bytes ?? new Uint8Array(capacity * elementSize);
  }

  remain() {
    return this.length - this.progress;
  }

  read(n) {
    const start = this.progress * this.elementSize;
    this.progress += n;
    return this.bytes.slice(start, start + n * this.elementSize);
  }

  write(bytes) {
    const start = this.progress * this.elementSize;
    this.bytes.set(bytes, start);
    this.progress += this.elementSize === 0 ? 1 : bytes.length / this.elementSize;
  }

  transferred(count) {
    return this.bytes.slice(0, count * this.elementSize);
  }
}

// One end of a stream or future, following `End` in the Canonical ABI. The
// state machine is: idle -> copying (a buffer is attached) -> idle/done once
// the pending event is *delivered* (the closure below runs), which is why the
// closures read `other` and `state` at delivery time rather than capture them.
class End extends Waitable {
  state = 'idle';
  buffer = null;
  other = null;
  index = null;
  t = null;

  constructor(eventCode) {
    super();
    this.eventCode = eventCode;
  }

  copy(buffer, isRead) {
    if (this.buffer !== null) {
      throw new Error('copy while a buffer is already pending');
    }
    this.state = 'copying';
    if (this.other === null) {
      if (!this.hasPendingEvent()) {
        throw new Error('copy on a dropped end without a pending event');
      }
    } else if (this.other.buffer === null) {
      // Nothing to rendezvous with yet; park the buffer.
      this.buffer = buffer;
    } else if (buffer.remain() > 0 && this.other.buffer.remain() > 0) {
      // Both sides are blocked on each other: copy the smaller amount and
      // notify both ends of the progress.
      const n = Math.min(buffer.remain(), this.other.buffer.remain());
      const src = isRead ? this.other.buffer : buffer;
      const dst = isRead ? buffer : this.other.buffer;
      dst.write(src.read(n));
      this.notify(buffer.progress);
      this.other.notify(this.other.buffer.progress);
      if (this.other.buffer.remain() === 0) {
        this.other.buffer = null;
      }
    } else if (buffer.remain() > 0 || (isRead && this.other.buffer.remain() === 0)) {
      // The other end is waiting on this one; hand the progress over and park
      // this end's buffer.
      this.other.notify(0);
      this.other.buffer = null;
      this.buffer = buffer;
    } else {
      // A zero-length write with a reading partner: complete immediately.
      this.notify(0);
    }
  }

  drop() {
    if (this.state === 'copying' || this.state === 'cancelling') {
      throw new Error(`drop while ${this.state}`);
    }
    if (this.other !== null) {
      this.other.other = null;
      if (this.other.state !== 'done' && !this.other.hasPendingEvent()) {
        this.other.notify(0);
      }
      this.other = null;
    }
    this.join(null);
  }
}

class StreamEnd extends End {
  notify(progress) {
    this.pendingEvent = () => {
      this.buffer = null;
      let result;
      if (this.other === null) {
        result = COPY_DROPPED;
        this.state = 'done';
      } else if (this.state === 'cancelling') {
        result = COPY_CANCELLED;
        this.state = 'idle';
      } else {
        result = COPY_COMPLETED;
        this.state = 'idle';
      }
      return [this.eventCode, this.index, result | (progress << 4)];
    };
  }
}

class FutureEnd extends End {
  notify(progress) {
    this.pendingEvent = () => {
      let result;
      if (progress === 1) {
        this.state = 'done';
        result = COPY_COMPLETED;
      } else if (this.other === null) {
        this.buffer = null;
        this.state = 'done';
        result = COPY_DROPPED;
      } else {
        this.buffer = null;
        this.state = 'idle';
        result = COPY_CANCELLED;
      }
      // Futures don't pack the element count: it is always 0 or 1.
      return [this.eventCode, this.index, result];
    };
  }
}

class ReadableStreamEnd extends StreamEnd {
  constructor() {
    super(EVENT_STREAM_READ);
  }
}

class WritableStreamEnd extends StreamEnd {
  constructor() {
    super(EVENT_STREAM_WRITE);
  }
}

class ReadableFutureEnd extends FutureEnd {
  constructor() {
    super(EVENT_FUTURE_READ);
  }
}

class WritableFutureEnd extends FutureEnd {
  constructor() {
    super(EVENT_FUTURE_WRITE);
  }
}

class HandleTable {
  #elems = new Map();
  #next = 1;

  add(entry) {
    const index = this.#next++;
    this.#elems.set(index, entry);
    if (entry instanceof End) {
      entry.index = index;
    }
    return index;
  }

  get(index) {
    const entry = this.#elems.get(index);
    if (entry === undefined) {
      throw new Error(`invalid handle ${index}`);
    }
    return entry;
  }

  remove(index) {
    const entry = this.get(index);
    this.#elems.delete(index);
    return entry;
  }
}

const READ_CHUNK = 4096;

function makeComponentRuntime(libc, collector) {
  const handles = new HandleTable();
  const drains = [];
  const decoder = new TextDecoder();
  let context = 0;
  // The waitable set of the task created by the `component_1` call currently
  // running. `canon.waitable-set.new` is only ever called by `spawnTask`, so
  // the most recent set is the task's set.
  let taskSetIndex = null;
  // Whether the async `invoke-test` export has returned (its `canon
  // task.return` fired).
  let taskReturned = false;

  function newPair(reader, writer, elementType) {
    reader.t = elementType;
    writer.t = elementType;
    reader.other = writer;
    writer.other = reader;
    const readable = handles.add(reader);
    const writable = handles.add(writer);
    // `stream.new`/`future.new` pack the readable end into the low 32 bits
    // and the writable end into the high 32 bits.
    return (BigInt(writable) << 32n) | BigInt(readable);
  }

  function canonCopy(EndClass, elementType, isRead, index, ptr, length) {
    const end = handles.get(index);
    if (!(end instanceof EndClass) || end.t !== elementType) {
      throw new Error(`handle ${index} is not the expected end kind`);
    }
    if (end.state !== 'idle') {
      throw new Error(`handle ${index} is ${end.state}, not idle`);
    }
    // These canon definitions are `async`, so an end that also sits in a
    // waitable set may copy; the pending event is then delivered through the
    // set instead of the return value.
    end.copy(new GuestBuffer(libc.memory, ptr, length, elementType.size), isRead);
    if (!end.hasPendingEvent()) {
      return BLOCKED;
    }
    const [, , payload] = end.getPendingEvent();
    return payload;
  }

  function dropEnd(EndClass, elementType, index) {
    const end = handles.remove(index);
    if (!(end instanceof EndClass) || end.t !== elementType) {
      throw new Error(`handle ${index} is not the expected end kind`);
    }
    end.drop();
  }

  function dropWritableFuture(elementType, index) {
    const end = handles.remove(index);
    if (!(end instanceof WritableFutureEnd) || end.t !== elementType) {
      throw new Error(`handle ${index} is not the expected end kind`);
    }
    // `canon future.drop-write` traps unless the end is DONE: the write must
    // have completed, or the readable end's DROPPED notification must have
    // been delivered first (see `drop` in the Canonical ABI).
    if (end.state !== 'done') {
      throw new Error(
        `future ${index} dropped writable before being written or dropped`,
      );
    }
    end.drop();
  }

  function emitStdout(chunks) {
    const total = chunks.reduce((n, chunk) => n + chunk.length, 0);
    const bytes = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) {
      bytes.set(chunk, offset);
      offset += chunk.length;
    }
    const lines = decoder.decode(bytes).split('\n');
    if (lines.length > 0 && lines[lines.length - 1] === '') {
      lines.pop();
    }
    collector.lines.push(...lines);
  }

  // Drains one printed stream: reads until the guest drops its writable end,
  // then completes the `write-via-stream` future with an `ok` result.
  class StdoutDrain {
    #readEnd;
    #futureWritable;
    #chunks = [];
    #buffer = null;
    #streamDone = false;
    #futureWritten = false;
    done = false;

    constructor(readEnd, futureWritable) {
      this.#readEnd = readEnd;
      this.#futureWritable = futureWritable;
    }

    // Runs the host side as far as it can. Returns true while the drain still
    // owes work (and is waiting for the guest to make progress).
    pump() {
      if (this.done) return false;
      while (!this.#streamDone) {
        if (this.#readEnd.hasPendingEvent()) {
          this.#deliverRead();
          continue;
        }
        if (this.#readEnd.state === 'idle') {
          this.#issueRead();
          continue;
        }
        return true;
      }
      if (!this.#futureWritten) {
        this.#futureWritten = true;
        // An `ok` result: tag byte 0, and no error code (see
        // `_WriteResultVtable.load` in tmp_print.dart).
        this.#futureWritable.copy(
          new HostBuffer(1, STDOUT_RESULT.size, Uint8Array.of(0, 0)),
          false,
        );
      }
      if (this.#futureWritable.hasPendingEvent()) {
        this.#futureWritable.getPendingEvent();
        this.#futureWritable.drop();
        this.done = true;
        emitStdout(this.#chunks);
        return false;
      }
      return true;
    }

    #issueRead() {
      this.#buffer = new HostBuffer(READ_CHUNK, U8.size);
      this.#readEnd.copy(this.#buffer, true);
    }

    #deliverRead() {
      const [code, index, payload] = this.#readEnd.getPendingEvent();
      const result = payload & 0xf;
      const progress = payload >>> 4;
      this.#chunks.push(this.#buffer.transferred(progress));
      if (result === COPY_COMPLETED) {
        // The end is idle again; `pump` issues the next read.
      } else if (result === COPY_DROPPED) {
        this.#streamDone = true;
        this.#readEnd.drop();
      } else {
        throw new Error(`unexpected CANCELLED on stdout stream read`);
      }
    }
  }

  function writeViaStream(streamIndex) {
    // Ownership of the readable end moves to the host (see `lift_stream` in
    // the Canonical ABI).
    const end = handles.remove(streamIndex);
    if (!(end instanceof ReadableStreamEnd) || end.t !== U8) {
      throw new Error(`handle ${streamIndex} is not a readable u8 stream end`);
    }
    if (end.state !== 'idle') {
      throw new Error(`stream ${streamIndex} is ${end.state}, not idle`);
    }
    if (end.wset !== null) {
      throw new Error(`stream ${streamIndex} is in a waitable set`);
    }
    const reader = new ReadableFutureEnd();
    const writer = new WritableFutureEnd();
    const readable = handles.add(reader);
    writer.t = STDOUT_RESULT;
    reader.t = STDOUT_RESULT;
    reader.other = writer;
    writer.other = reader;
    drains.push(new StdoutDrain(end, writer));
    return readable;
  }

  function beginTask() {
    taskSetIndex = null;
    taskReturned = false;
  }

  // The guest runs a case inside a task; its event loop is driven through the
  // exported `callback` function. A task returns `wait` with the index of its
  // waitable set whenever it still has work, and the pump delivers pending
  // events (running host-side drains in between) until the set is empty.
  function driveTask(instance) {
    if (taskSetIndex === null) {
      throw new Error('the test case did not create a task waitable set');
    }
    let setIndex = taskSetIndex;
    for (;;) {
      for (const drain of drains) {
        drain.pump();
      }
      for (let i = drains.length - 1; i >= 0; i--) {
        if (drains[i].done) drains.splice(i, 1);
      }

      const set = handles.get(setIndex);
      const ready = set.findReady();
      if (ready !== null) {
        const callback = instance.exports.callback;
        if (typeof callback !== 'function') {
          throw new Error(
            'pending task events but the module does not export `callback`',
          );
        }
        const [code, p1, p2] = ready.getPendingEvent();
        const packed = callback(code, p1, p2) >>> 0;
        const code4 = packed & 0xf;
        setIndex = packed >>> 4;
        if (code4 === CALLBACK_EXIT) return;
        continue;
      }
      if (set.elems.length === 0) return;
      throw new Error(
        `task stalled: ${set.elems.length} waitable(s) in set ${setIndex} ` +
          `with no pending events`,
      );
    }
  }

  const imports = {
    'canon.context.get_i32_0': () => context,
    'canon.context.set_i32_0': (value) => {
      context = value;
    },
    'canon.waitable-set.new': () => {
      taskSetIndex = handles.add(new WaitableSet());
      return taskSetIndex;
    },
    'canon.waitable.join': (waitable, set) => {
      handles.get(waitable).join(handles.get(set));
    },

    // Subtasks are created by async imports (timers lower to a
    // `wasi:clocks/monotonic-clock.wait-for` subtask). No case here starts one
    // at run time, but the timer machinery is compiled in whenever it is
    // reachable, so the import must exist.
    'canon.subtask.drop': (subtask) => {
      handles.remove(subtask).join(null);
    },

    // `canon task.return` for the async `invoke-test` export: the generated
    // wrapper calls it when the task's body has finished. Nothing joins the
    // exported task, so completing it is all the host has to do; calling it
    // twice would trap, matching the Canonical ABI.
    _component_1taskReturn: () => {
      if (taskReturned) {
        throw new Error('canon task.return called twice');
      }
      taskReturned = true;
    },

    // The task/subtask machinery of the runtime uses `future<void>`.
    'canon.future<void>.new': () => newPair(new ReadableFutureEnd(), new WritableFutureEnd(), VOID),
    'canon.future<void>.read': (future, ptr) =>
      canonCopy(ReadableFutureEnd, VOID, true, future, ptr, 1),
    'canon.future<void>.write': (future, ptr) =>
      canonCopy(WritableFutureEnd, VOID, false, future, ptr, 1),
    'canon.future<void>.drop-read': (future) =>
      dropEnd(ReadableFutureEnd, VOID, future),
    'canon.future<void>.drop-write': (future) =>
      dropWritableFuture(VOID, future),

    // The embedder's `print` (tmp_print.dart) uses these to talk to
    // `wasi:cli/stdout`.
    'implicitImport_stdoutStreamNew': () =>
      newPair(new ReadableStreamEnd(), new WritableStreamEnd(), U8),
    'implicitImport_stdoutStreamRead': (stream, ptr, count) =>
      canonCopy(ReadableStreamEnd, U8, true, stream, ptr, count),
    'implicitImport_stdoutStreamWrite': (stream, ptr, count) =>
      canonCopy(WritableStreamEnd, U8, false, stream, ptr, count),
    'implicitImport_stdoutStreamDropReadable': (stream) =>
      dropEnd(ReadableStreamEnd, U8, stream),
    'implicitImport_stdoutStreamDropWritable': (stream) =>
      dropEnd(WritableStreamEnd, U8, stream),
    'implicitImport_stdoutFutureNew': () =>
      newPair(new ReadableFutureEnd(), new WritableFutureEnd(), STDOUT_RESULT),
    'implicitImport_stdoutFutureRead': (future, ptr) =>
      canonCopy(ReadableFutureEnd, STDOUT_RESULT, true, future, ptr, 1),
    'implicitImport_stdoutFutureWrite': (future, ptr) =>
      canonCopy(WritableFutureEnd, STDOUT_RESULT, false, future, ptr, 1),
    'implicitImport_stdoutFutureDropReadable': (future) =>
      dropEnd(ReadableFutureEnd, STDOUT_RESULT, future),
    'implicitImport_stdoutFutureDropWritable': (future) =>
      dropWritableFuture(STDOUT_RESULT, future),
    'implicitImport_stdoutWriteViaStream': writeViaStream,

    _import0: (ptr, packedLength) => {
      collector.lines.push(
        JSON.stringify({
          type: 'string',
          value: readUtf16(libc.memory, ptr, packedLength),
        }),
      );
    },
    _import1: collector.recordDouble,
    _import2: collector.recordInt,
    _import3: collector.recordBool,
  };

  return { imports, beginTask, driveTask };
}

function runCase(dartFile, wasmFile) {
  compile(dartFile, wasmFile);
  const compiled = fs.readFileSync(wasmFile);

  const names = functionNames(compiled);
  const mainIndex = Object.entries(names).find(
    ([, name]) => name === 'main',
  )?.[0];
  if (mainIndex == null) {
    throw new Error('Could not find the `main` function in the name section.');
  }
  const module = new WebAssembly.Module(
    exportFunction(compiled, Number(mainIndex), 'main'),
  );

  const importInfos = WebAssembly.Module.imports(module);
  const unknownModules = importInfos
    .map((info) => info.module)
    .filter((m) => !['dart', 'libc', 'component'].includes(m));
  if (unknownModules.length > 0) {
    throw new Error(
      `Unexpected import modules: ${[...new Set(unknownModules)].join(', ')}`,
    );
  }

  const libc = makeLibc();
  const collector = makeCollector();

  let instance = null;
  // Every `dart.*` import must be provided by an export of the same module.
  // Resolve the export on every call so that linking failures surface as
  // errors naming the missing import.
  const dart = {};
  for (const name of new Set(
    importInfos.filter((info) => info.module === 'dart').map((i) => i.name),
  )) {
    dart[name] = (...args) => {
      const implementation = instance?.exports[name];
      if (implementation == null) {
        throw new Error(
          `The import dart.${name} has no wasm:export implementation in the test module.`,
        );
      }
      return implementation(...args);
    };
  }

  // A miniature component runtime: the handle table, the stream/future copy
  // protocol, waitable sets and the `callback` event pump that a task needs
  // (see `makeComponentRuntime` above).
  const runtime = makeComponentRuntime(libc, collector);
  const component = runtime.imports;
  // The collector imports are named _import0.._import3 in the generated
  // bindings. Any import without a host implementation fails instantiation
  // with a clear message below.
  const componentImports = {};
  for (const info of importInfos.filter((info) => info.module === 'component')) {
    const implementation = component[info.name];
    if (implementation == null) {
      throw new Error(`No host implementation for component.${info.name}`);
    }
    componentImports[info.name] = implementation;
  }

  const caseName = path.basename(dartFile);
  // Every phase below can trap. Wrapping each one names the phase in the
  // error output, which turns "[object WebAssembly.Exception]" from the
  // top-level handler into a usable pointer.
  try {
    instance = new WebAssembly.Instance(module, {
      dart,
      libc: {
        memory: libc.memory,
        dart_realloc: libc.realloc,
        dart_free: libc.free,
      },
      component: componentImports,
    });
  } catch (error) {
    console.error(`FAIL ${caseName}: instantiation threw`, String(error));
    throw error;
  }

  // Runs `main`, which registers the test cases.
  try {
    instance.exports.main();
  } catch (error) {
    console.error(`FAIL ${caseName}: main threw`, String(error));
    throw error;
  }

  const count = instance.exports.component_0();
  const lines = collector.lines;
  for (let i = 0; i < count; i++) {
    lines.push(`{"type":"start","test":${i}}`);
    // `component_1` runs the case inside a task (spawnTask). The call returns
    // once the case's synchronous part is done; async work is then driven by
    // the event pump below.
    runtime.beginTask();
    try {
      instance.exports.component_1(i);
    } catch (error) {
      console.error(`FAIL ${caseName}: test ${i} threw`, String(error));
      throw error;
    }
    // Flush the task: deliver pending events until its waitable set is empty.
    // Printed output reaches the collector only once its drain finishes, so
    // this must happen before the `end` marker.
    try {
      runtime.driveTask(instance);
    } catch (error) {
      console.error(`FAIL ${caseName}: test ${i} stalled`, String(error));
      throw error;
    }
    lines.push(`{"type":"end","test":${i}}`);
  }
  return `${lines.join('\n')}\n`;
}

const cases = fs
  .readdirSync(casesDir)
  .filter((name) => name.endsWith('.dart'))
  .sort();

if (cases.length === 0) {
  throw new Error(`No test cases found in ${casesDir}`);
}

let failed = false;
for (const name of cases) {
  const dartFile = path.join(casesDir, name);
  const goldenFile = path.join(casesDir, name.replace(/\.dart$/, '.golden.txt'));
  const wasmFile = path.join(casesDir, name.replace(/\.dart$/, '.wasm'));
  try {
    const actual = runCase(dartFile, wasmFile);
    const expected = fs.readFileSync(goldenFile, 'utf8');
    if (actual === expected) {
      console.log(`PASS ${name}`);
    } else {
      failed = true;
      console.error(
        `FAIL ${name}: output does not match ${path.basename(goldenFile)}`,
      );
      console.error('--- expected ---');
      console.error(expected);
      console.error('--- actual ---');
      console.error(actual);
    }
  } catch (error) {
    failed = true;
    console.error(`FAIL ${name}:`, String(error));
  } finally {
    fs.rmSync(wasmFile, { force: true });
  }
}

process.exitCode = failed ? 1 : 0;
