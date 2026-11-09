import 'dart:io';

import 'package:test/test.dart';

import '../../test_runner/testdata/plugin_probe/entity_decode.dart';

/// Native side of the entity-decode native/wasm parity proof (issue #85
/// Phase 3). The same vectors are asserted against the compiled wasm
/// component by `pkg/test_runner/src/plugin_loader.rs`
/// (`entity_decode_component_matches_native_parity_vectors`).
const vectors = <String, String>{
  'a&amp;b': 'a&b',
  '&lt;div&gt;': '<div>',
  '&quot;hi&quot;': '"hi"',
  '&apos;it&apos;s': "'it's",
  'caf\u00e9&nbsp;x': 'caf\u00e9\u00a0x',
  '&#65;&#x42;&#x43;': 'ABC',
  '&unknown; stays': '&unknown; stays',
  'bare &amp tail': 'bare &amp tail',
  'double &amp;amp;': 'double &amp;',
  '': '',
  'plain text': 'plain text',
  '&#x110000; bad': '&#x110000; bad',
  'a&b': 'a&b',
};

void main() {
  // The wasm component's bytes come from the same authoring source; assert
  // the component exists so the parity pair cannot drift apart silently.
  // `dart test` may run with the package or the workspace as the working
  // directory, so look one level up as well.
  test('wasm fixture is committed next to the native source', () {
    const rel = 'test_runner/testdata/plugin_probe/entity_decode.component.wasm';
    final cwd = Directory.current.path;
    final root = Directory(cwd).parent.path;
    final found = [
      File('$cwd/pkg/$rel'),
      File('$cwd/$rel'),
      File('$root/pkg/$rel'),
      File('$root/$rel'),
    ].firstWhere((file) => file.existsSync(), orElse: () => File('$cwd/pkg/$rel'));
    expect(found.existsSync(), isTrue,
        reason: 'run regenerate_fixture.sh to build entity_decode.component.wasm '
            '(looked under $cwd and $root)');
  });

  test('native decodeEntities matches the parity vectors', () {
    vectors.forEach((input, expected) {
      expect(decodeEntities(input), expected, reason: 'input: $input');
    });
  });
}
