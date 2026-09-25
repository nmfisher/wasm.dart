// Exports functions imported by the Dart SDK in
// https://github.com/dart-lang/sdk/blob/main/sdk/lib/_internal/wasm/standalone/embedder.dart.
// After invoking dart2wasm, we merge imports against these definitions.
library;

// ignore: import_internal_library
import 'dart:_wasm';

import '../runtime/async/task.dart';
import 'clock.dart';
import 'constants.dart';
import 'double_format.dart';
import 'double_parse.dart';
import 'json_encode.dart';
import 'number_format.dart';
import 'regexp.dart';
import 'stack_trace.dart';
import 'string.dart';
import 'string_buffer.dart';
import 'tmp_print.dart';
import 'utils.dart';

Never _unsupportedAsyncSchedule() {
  throw StateError('Tried to schedule async operation, outside of async task.');
}

@pragma('wasm:export')
WasmExternRef scheduleOnce(
  WasmI64 delay,
  WasmFunction<WasmVoid Function(WasmAnyRef)> callback,
  WasmAnyRef arg,
) {
  _unsupportedAsyncSchedule();
}

@pragma('wasm:export')
WasmExternRef scheduleRepeated(
  WasmI64 interval,
  WasmFunction<WasmVoid Function(WasmAnyRef)> callback,
  WasmAnyRef arg,
) {
  _unsupportedAsyncSchedule();
}

@pragma('wasm:export')
WasmVoid queueMicrotask(
  WasmFunction<WasmVoid Function(WasmAnyRef)> callback,
  WasmAnyRef arg,
) {
  // Ideally, this should not get called as all async work happens in a task
  // zone overriding scheduleMicrotask.
  // However, the Dart SDK uses a null future instance used in some cases:
  // https: //github.com/dart-lang/sdk/blob/f300393bdf6136f4be35d877763ab73c7c715647/sdk/lib/internal/internal.dart#L140-L160
  // To support that, schedule that on the last that ran.
  final task = Task.forCurrentThreadUnchecked();
  if (task != null) {
    task.scheduleRawMicrotask(() {
      callback.call(arg);
    });
    return WasmVoid();
  }

  _unsupportedAsyncSchedule();
}

@pragma('wasm:export')
WasmVoid clearSchedule(WasmExternRef? schedule) {
  _unsupportedAsyncSchedule();
}

@pragma('wasm:export')
WasmExternRef stringFromAsciiBytes(
  WasmArray<WasmI8> charCodes,
  WasmI32 start,
  WasmI32 length,
) {
  return WasmAnyRef.fromObject(
    Latin1String.fromAsciiBytes(charCodes, start, length),
  ).externalize();
}

@pragma('wasm:export')
WasmExternRef stringFromCharCodeArray(
  WasmArray<WasmI16> charCodes,
  WasmI32 start,
  WasmI32 length,
) {
  return WasmAnyRef.fromObject(
    Utf16String.fromCharCodes(charCodes, start, length),
  ).externalize();
}

@pragma('wasm:export')
WasmExternRef i64ToString(WasmI64 value, WasmI32 radix) {
  return intToString(value.toInt(), radix.toIntUnsigned()).externalize();
}

@pragma('wasm:export')
WasmExternRef f64ToString(WasmF64 value) {
  return doubleToString(value.toDouble()).externalize();
}

@pragma('wasm:export')
WasmExternRef f64ToExponential(WasmF64 value) {
  return doubleToExponentialWithFractionDigits(value.toDouble(), -1)
      .externalize();
}

@pragma('wasm:export')
WasmExternRef f64ToExponentialWithFractionDigits(WasmF64 value, WasmI32 digits) {
  return doubleToExponentialWithFractionDigits(
    value.toDouble(),
    digits.toIntSigned(),
  ).externalize();
}

@pragma('wasm:export')
WasmExternRef f64ToPrecision(WasmF64 value, WasmI32 digits) {
  return doubleToPrecision(value.toDouble(), digits.toIntSigned())
      .externalize();
}

@pragma('wasm:export')
WasmExternRef f64ToFixed(WasmF64 value, WasmI32 digits) {
  return doubleToFixed(value.toDouble(), digits.toIntSigned()).externalize();
}

final class _DoubleTryParseResult {
  final double value;

  _DoubleTryParseResult(this.value);
}

@pragma('wasm:export')
WasmExternRef? doubleTryParse(WasmExternRef? string) {
  final result = tryParseDouble(WasmStringImplementation.fromExtern(string));
  if (result is DoubleParseSuccess) {
    return WasmAnyRef.fromObject(_DoubleTryParseResult(result.value))
        .externalize();
  }
  return WasmExternRef.nullRef;
}

@pragma('wasm:export')
WasmF64 tryParseResultGetDouble(WasmExternRef? parseResult) {
  final result =
      parseResult!.internalize().toObject() as _DoubleTryParseResult;
  return WasmF64.fromDouble(result.value);
}

@pragma('wasm:export')
WasmF64 doubleParseInfallible(WasmExternRef? string) {
  final result = tryParseDouble(WasmStringImplementation.fromExtern(string));
  if (result is DoubleParseSuccess) {
    return WasmF64.fromDouble(result.value);
  }
  // The SDK only calls this on strings it has already validated (e.g. JSON
  // number tokens). Returning zero keeps a bad call non-fatal.
  return const WasmF64(0.0);
}

@pragma('wasm:export')
WasmI32 stringLength(WasmExternRef? string) {
  return WasmStringImplementation.fromExtern(string).length.toWasmI32();
}

@pragma('wasm:export')
WasmI32 stringEquals(WasmExternRef? a, WasmExternRef? b) {
  return WasmI32.fromBool(
    WasmStringImplementation.fromExtern(a)
        .stringEquals(WasmStringImplementation.fromExtern(b)),
  );
}

@pragma('wasm:export')
WasmI32 stringCompare(WasmExternRef? a, WasmExternRef? b) {
  return WasmStringImplementation.fromExtern(a)
      .compareTo(WasmStringImplementation.fromExtern(b));
}

@pragma('wasm:export')
WasmI32 stringCodeUnitAt(WasmExternRef? string, WasmI32 index) {
  final wasmString = WasmStringImplementation.fromExtern(string);
  return WasmI32.fromInt(wasmString.codeUnitAtUnchecked(index.toIntUnsigned()));
}

@pragma('wasm:export')
WasmExternRef? stringSubstring(
  WasmExternRef? string,
  WasmI32 start,
  WasmI32 end,
) {
  return WasmStringImplementation.fromExtern(string)
      .substring(start, end)
      .externalize();
}

@pragma('wasm:export')
WasmI32 stringIndexOfString(WasmExternRef? a, WasmExternRef? b, WasmI32 start) {
  return WasmI32.fromInt(
    WasmStringImplementation.fromExtern(a).indexOfString(
      WasmStringImplementation.fromExtern(b),
      start.toIntSigned(),
    ),
  );
}

@pragma('wasm:export')
WasmI32 stringLastIndexOfString(
  WasmExternRef? a,
  WasmExternRef? b,
  WasmI32 start,
) {
  return WasmI32.fromInt(
    WasmStringImplementation.fromExtern(a).lastIndexOfString(
      WasmStringImplementation.fromExtern(b),
      start.toIntSigned(),
    ),
  );
}

@pragma('wasm:export')
WasmExternRef? stringToLowerCase(WasmExternRef? string) {
  return WasmStringImplementation.fromExtern(string).toLower().externalize();
}

@pragma('wasm:export')
WasmExternRef? stringToUpperCase(WasmExternRef? string) {
  return WasmStringImplementation.fromExtern(string).toUpper().externalize();
}

@pragma('wasm:export')
WasmExternRef? stringConcat(WasmExternRef? a, WasmExternRef? b) {
  return WasmStringImplementation.fromExtern(a)
      .concat(WasmStringImplementation.fromExtern(b))
      .externalize();
}

@pragma('wasm:export')
WasmExternRef? stringRepeat(WasmExternRef? string, WasmI32 amount) {
  final wasmString = WasmStringImplementation.fromExtern(string);
  return wasmString.repeat(amount.toIntSigned()).externalize();
}

@pragma('wasm:export')
WasmExternRef? stringReplaceAllString(
  WasmExternRef? string,
  WasmExternRef? needle,
  WasmExternRef? replacement,
) {
  return WasmStringImplementation.fromExtern(string)
      .replaceAllString(
        WasmStringImplementation.fromExtern(needle),
        WasmStringImplementation.fromExtern(replacement),
      )
      .externalize();
}

@pragma('wasm:export')
WasmExternRef? stringReplaceRange(
  WasmExternRef? string,
  WasmI32 start,
  WasmI32 end,
  WasmExternRef? replacement,
) {
  final wasmString = WasmStringImplementation.fromExtern(string);
  final part = WasmStringImplementation.fromExtern(replacement);
  return wasmString
      .substring(const WasmI32(0), start)
      .concat(part)
      .concat(wasmString.substring(end, WasmI32.fromInt(wasmString.length)))
      .externalize();
}

@pragma('wasm:export')
WasmVoid stringToCodeUnits(
  WasmExternRef? string,
  WasmArray<WasmI16> outArray,
  WasmI32 startIndex,
) {
  WasmStringImplementation.fromExtern(string).writeToCodeUnits(
    outArray,
    startIndex.toIntUnsigned(),
  );
  return WasmVoid();
}

@pragma('wasm:export')
WasmExternRef stringBufferCreate() {
  return WasmStringBuffer().externalize();
}

@pragma('wasm:export')
WasmVoid stringBufferWriteString(WasmExternRef? buffer, WasmExternRef? string) {
  (buffer!.internalize().toObject() as WasmStringBuffer).writeString(
    WasmStringImplementation.fromExtern(string),
  );
  return WasmVoid();
}

@pragma('wasm:export')
WasmVoid stringBufferWriteCharCode(WasmExternRef? buffer, WasmI32 code) {
  (buffer!.internalize().toObject() as WasmStringBuffer).writeCharCode(
    code.toIntUnsigned(),
  );
  return WasmVoid();
}

@pragma('wasm:export')
WasmVoid stringBufferClear(WasmExternRef? buffer) {
  (buffer!.internalize().toObject() as WasmStringBuffer).clear();
  return WasmVoid();
}

@pragma('wasm:export')
WasmI32 stringBufferLength(WasmExternRef? buffer) {
  return (buffer!.internalize().toObject() as WasmStringBuffer).length
      .toWasmI32();
}

@pragma('wasm:export')
WasmExternRef stringBufferToString(WasmExternRef? buffer) {
  return (buffer!.internalize().toObject() as WasmStringBuffer)
      .renderToString()
      .externalize();
}

@pragma('wasm:export')
WasmExternRef stackTraceGetCurrent() {
  // WASI doesn't expose stack traces, so this is unimplemented.
  return const UnsupportedStackTrace().externalize();
}

@pragma('wasm:export')
WasmExternRef stackTraceToString(WasmExternRef? _) {
  return stackTracesAreUnavailableMessage.externalize();
}

@pragma('wasm:export')
WasmExternRef jsonEncodeString(WasmExternRef? line) {
  return jsonEncodeStringImpl(WasmStringImplementation.fromExtern(line))
      .externalize();
}

@pragma('wasm:export')
WasmVoid debugger(WasmExternRef? message) {
  return WasmVoid();
}

@pragma('wasm:export', 'print')
WasmVoid wasiPrint(WasmExternRef? string) {
  printImpl(.fromExtern(string));
  return WasmVoid();
}

@pragma('wasm:export', 'randomInt')
WasmI64 randomInt() {
  // Note: This function is recognized by the component compiler, which will add
  // a dependency on wasi:random/insecure to replace this function with a
  // get-insecure-random-u64 import.
  throw UnsupportedError('wasi:random/insecure not available');
}

@pragma('wasm:export', 'randomIntSecure')
WasmI64 randomIntSecure() {
  // Note: This function is recognized by the component compiler, which will add
  // a dependency on wasi:random/insecure to replace this function with a
  // get-random-u64 import.
  throw UnsupportedError('wasi:random/random not available');
}

@pragma('wasm:export', 'currentTime')
WasmI64 currentTimeMicros() {
  return WasmI64.fromInt(wasiTimestampInMicroseconds());
}

@pragma("wasm:export", "timeZoneNameForClampedSeconds")
WasmExternRef timeZoneNameForClampedSeconds(WasmI64 secondsSinceEpoch) {
  // We can't get the time zone name without including a tz db in our modules.
  // Instead, we return the (time-independent) id of the time zone.
  return wasiIanaId().externalize();
}

@pragma('wasm:export', 'timeZoneOffsetInSecondsForClampedSeconds')
WasmI32 timeZoneOffsetInSecondsForClampedSeconds(WasmI64 secondsSinceEpoch) {
  return const WasmI32(0);
}

@pragma('wasm:export', 'monotonicClockFrequency')
WasmI32 monotonicClockFrequency() {
  return dartStopwatchTickFrequency.toWasmI32();
}

@pragma('wasm:export', 'monotonicClockTicks')
WasmI64 monotonicClockTicks() {
  return dartMonotonicTicks.toWasmI64();
}

// ---------------------------------------------------------------------------
// dart.regexp* — implemented in embedder/regexp.dart
// ---------------------------------------------------------------------------

/// Compiled regexps and matches cross the wasm boundary as opaque objects,
/// exactly like `_DoubleTryParseResult` above. The SDK distinguishes a
/// successful compile from an error-string result with [regexpIsRegexp].
@pragma('wasm:export', 'regexpCreateOrFailWithString')
WasmExternRef regexpCreateOrFailWithString(
  WasmExternRef? string,
  WasmI32 multiLine,
  WasmI32 caseSensitive,
  WasmI32 unicode,
  WasmI32 dotAll,
) {
  final compiled = EmbedderRegexp.compile(
    WasmStringImplementation.fromExtern(string).toDartString(),
    multiLine.toBool(),
    caseSensitive.toBool(),
    unicode.toBool(),
    dotAll.toBool(),
  );
  return WasmAnyRef.fromObject(compiled).externalize();
}

@pragma('wasm:export', 'regexpIsRegexp')
WasmI32 regexpIsRegexp(WasmExternRef? ref) {
  return WasmI32.fromBool(ref!.internalize().toObject() is EmbedderRegexp);
}

@pragma('wasm:export', 'regexpEscape')
WasmExternRef regexpEscape(WasmExternRef? string) {
  return EmbedderRegexp.escape(WasmStringImplementation.fromExtern(string))
      .externalize();
}

@pragma('wasm:export', 'regexpMatch')
WasmExternRef? regexpMatch(
  WasmExternRef? regexp,
  WasmExternRef? string,
  WasmI32 start,
  WasmI32 asPrefix,
) {
  final pattern = regexp!.internalize().toObject() as EmbedderRegexp;
  final match = pattern.match(
    WasmStringImplementation.fromExtern(string),
    start.toIntUnsigned(),
    asPrefix.toBool(),
  );
  if (match == null) return WasmExternRef.nullRef;
  return WasmAnyRef.fromObject(match).externalize();
}

@pragma('wasm:export', 'regexpMatchGetStart')
WasmI32 regexpMatchGetStart(WasmExternRef? match) {
  final m = match!.internalize().toObject() as EmbedderRegexpMatch;
  return WasmI32.fromInt(m.start);
}

@pragma('wasm:export', 'regexpMatchGetEnd')
WasmI32 regexpMatchGetEnd(WasmExternRef? match) {
  final m = match!.internalize().toObject() as EmbedderRegexpMatch;
  return WasmI32.fromInt(m.end);
}

@pragma('wasm:export', 'regexpMatchGetGroupCount')
WasmI32 regexpMatchGetGroupCount(WasmExternRef? match) {
  final m = match!.internalize().toObject() as EmbedderRegexpMatch;
  return WasmI32.fromInt(m.groupCount);
}

@pragma('wasm:export', 'regexpMatchGetGroup')
WasmExternRef? regexpMatchGetGroup(WasmExternRef? match, WasmI32 index) {
  final m = match!.internalize().toObject() as EmbedderRegexpMatch;
  final group = m.group(index.toIntUnsigned());
  if (group == null) return WasmExternRef.nullRef;
  return group.externalize();
}

@pragma('wasm:export', 'regexpMatchGetNamedGroups')
WasmI32 regexpMatchGetNamedGroups(WasmExternRef? match) {
  final m = match!.internalize().toObject() as EmbedderRegexpMatch;
  return WasmI32.fromInt(m.pattern.groupIndicesByName.length);
}

@pragma('wasm:export', 'regexpMatchGetGroupName')
WasmExternRef regexpMatchGetGroupName(WasmExternRef? match, WasmI32 index) {
  final m = match!.internalize().toObject() as EmbedderRegexpMatch;
  // SDK contract: [index] is between 0 and namedGroups (exclusive), counting
  // only the *named* groups in capture-order — not the slot in groupNames
  // (index 0 there is the unnamed whole match).
  var remaining = index.toIntUnsigned();
  for (var i = 1; i < m.pattern.groupNames.length; i++) {
    final name = m.pattern.groupNames[i];
    if (name == null) continue;
    if (remaining == 0) {
      // The parser stores plain Dart strings; the SDK expects one of our
      // string implementations on the other side of the boundary.
      return WasmStringImplementation.fromDartString(name).externalize();
    }
    remaining--;
  }
  // SDK contract: never called with an out-of-range index; guard anyway so a
  // bad call stays non-fatal.
  return Latin1String.empty.externalize();
}

@pragma('wasm:export', 'regexpMatchGetGroupByName')
WasmExternRef? regexpMatchGetGroupByName(WasmExternRef? match, WasmI32 nameIndex) {
  final m = match!.internalize().toObject() as EmbedderRegexpMatch;
  // Same index space as regexpMatchGetGroupName: the nameIndex-th named group.
  var remaining = nameIndex.toIntUnsigned();
  String? name;
  for (var i = 1; i < m.pattern.groupNames.length; i++) {
    final candidate = m.pattern.groupNames[i];
    if (candidate == null) continue;
    if (remaining == 0) {
      name = candidate;
      break;
    }
    remaining--;
  }
  final groupIndex = name == null ? null : m.pattern.groupIndicesByName[name];
  if (groupIndex == null) return WasmExternRef.nullRef;
  final group = m.group(groupIndex);
  if (group == null) return WasmExternRef.nullRef;
  return group.externalize();
}

@pragma('wasm:export', 'stringReplaceAllRegExp')
WasmExternRef stringReplaceAllRegExp(
  WasmExternRef? string,
  WasmExternRef? needle,
  WasmExternRef? replacement,
) {
  final pattern = needle!.internalize().toObject() as EmbedderRegexp;
  return pattern
      .replaceAllRegExp(
        WasmStringImplementation.fromExtern(string),
        WasmStringImplementation.fromExtern(replacement),
      )
      .externalize();
}
