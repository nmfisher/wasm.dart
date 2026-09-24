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

  const component = {
    'canon.context.get_i32_0': () => 0,
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

  instance = new WebAssembly.Instance(module, {
    dart,
    libc: {
      memory: libc.memory,
      dart_realloc: libc.realloc,
      dart_free: libc.free,
    },
    component: componentImports,
  });

  // Runs `main`, which registers the test cases.
  instance.exports.main();

  const count = instance.exports.component_0();
  const lines = collector.lines;
  for (let i = 0; i < count; i++) {
    lines.push(`{"type":"start","test":${i}}`);
    instance.exports.component_1(i);
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
