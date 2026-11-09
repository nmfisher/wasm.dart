import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import 'package:args/command_runner.dart';
import 'package:logging/logging.dart';

import '../failure.dart';

/// Appends the loader's embedder-ABI stamp (a single custom section carrying
/// `tina.embedder-abi` and a little-endian u32 version) to a Wasm component.
///
/// This is the Dart form of the authoring stamp previously only available as
/// `testdata/plugin_probe/stamp_abi.py`; the byte layout is identical, so
/// both produce the same file for the same input.
final class StampAbiCommand extends Command<void> {
  final Logger logger;

  StampAbiCommand(this.logger) {
    argParser
      ..addOption('output', abbr: 'o', help: 'Output component path.', mandatory: true)
      ..addOption(
        'abi-version',
        help: 'Embedder ABI version to stamp (little-endian u32).',
        defaultsTo: '1',
      )
      ..addOption(
        'abi-name',
        help: 'Custom-section name the loader validates.',
        defaultsTo: 'tina.embedder-abi',
        hide: true,
      );
  }

  @override
  String get description =>
      'Stamp a Wasm component with the plugin loader embedder-ABI section.';

  @override
  String get name => 'stamp-abi';

  static const _header = <int>[0, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];

  List<int> _uleb(int value) {
    final out = BytesBuilder();
    var remaining = value;
    while (remaining >= 0x80) {
      out.add([(remaining & 0x7f) | 0x80]);
      remaining >>= 7;
    }
    out.add([remaining]);
    return out.takeBytes();
  }

  (int, int) _readUleb(List<int> data, int pos) {
    var value = 0;
    for (var shift = 0; shift < 35; shift += 7) {
      if (pos >= data.length) {
        throw ToolFailure('truncated section length');
      }
      final byte = data[pos++];
      if (shift == 28 && byte > 0x0f) {
        throw ToolFailure('overflowing section length');
      }
      value |= (byte & 0x7f) << shift;
      if (byte < 0x80) {
        return (value, pos);
      }
    }
    throw ToolFailure('oversized section length');
  }

  int _existingStampCount(List<int> data, List<int> name) {
    if (data.length < _header.length || !_regionEquals(data, 0, _header.length, _header)) {
      throw ToolFailure('not a version-1 Wasm component');
    }
    var count = 0;
    var pos = _header.length;
    while (pos < data.length) {
      final sectionId = data[pos];
      final (length, body) = _readUleb(data, pos + 1);
      final end = body + length;
      if (end > data.length) {
        throw ToolFailure('section exceeds file');
      }
      if (sectionId == 0) {
        final (nameLength, namePos) = _readUleb(data, body);
        final nameEnd = namePos + nameLength;
        if (nameEnd > end) {
          throw ToolFailure('custom section name exceeds section');
        }
        if (_regionEquals(data, namePos, nameEnd, name)) {
          count += 1;
        }
      }
      pos = end;
    }
    return count;
  }

  bool _regionEquals(List<int> data, int start, int end, List<int> other) {
    if (end - start != other.length) {
      return false;
    }
    for (var i = 0; i < other.length; i++) {
      if (data[start + i] != other[i]) {
        return false;
      }
    }
    return true;
  }
  @override
  Future<void> run() async {
    final results = argResults!;
    final input = results.rest.isEmpty
        ? throw ToolFailure('an input component path is required')
        : results.rest.first;
    final output = results.option('output')!;
    final abiVersion = int.tryParse(results.option('abi-version')!);
    if (abiVersion == null || abiVersion < 0 || abiVersion > 0xffffffff) {
      throw ToolFailure('invalid --abi-version');
    }
    final name = results.option('abi-name')!.codeUnits;

    final data = await File(input).readAsBytes();
    if (_existingStampCount(data, name) != 0) {
      throw ToolFailure('input already has an embedder-ABI stamp');
    }
    final versionBytes = [
      abiVersion & 0xff,
      (abiVersion >> 8) & 0xff,
      (abiVersion >> 16) & 0xff,
      (abiVersion >> 24) & 0xff,
    ];
    final payload = <int>[..._uleb(name.length), ...name, ...versionBytes];
    final section = <int>[0, ..._uleb(payload.length), ...payload];

    final destination = File(output);
    await destination.writeAsBytes([...data, ...section], mode: FileMode.write);
    final stamped = await destination.readAsBytes();
    if (_existingStampCount(stamped, name) != 1) {
      throw ToolFailure('stamp verification failed');
    }
    logger.info('Stamped $input -> $output (embedder ABI $abiVersion)');
  }
}
