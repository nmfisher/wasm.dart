import 'entity_decode.dart' show decodeEntities;

import 'package:wasm_components/wasm_components.dart'
    show Borrowed, ErrorResult, OkResult, Result;

import 'package:issue85_plugin_probe/src/generated/tina_plugin_probe.dart' as wit;
import 'package:issue85_plugin_probe/src/generated/tina_plugin_probe_plugin_component.dart'
    show PluginComponentImports, pluginComponentComponent;

/// Stateless real plugin for the Phase 3 migration lane: decodes HTML
/// entities in the prompt payload. It keeps no state and calls no engine
/// service, so it exercises the narrow grant-free migration path.
final class EntityDecodePlugin implements wit.Plugin {
  @override
  Result<String, wit.PluginPluginError> run({
    required Borrowed<wit.ServiceTypesService> service,
    required String input,
  }) {
    return Result.ok(decodeEntities(input));
  }

  @override
  ({String id, String description}) describe() => (
    id: 'example/entity-decode',
    description: 'Decodes HTML entities in the payload; stateless.',
  );
}

void main() {
  pluginComponentComponent(
    (PluginComponentImports imports) => EntityDecodePlugin(),
  );
}
