import 'package:wasm_components/wasm_components.dart'
    show Borrowed, ErrorResult, OkResult, Result;

import 'package:issue85_plugin_probe/src/generated/tina_plugin_probe.dart' as wit;
import 'package:issue85_plugin_probe/src/generated/tina_plugin_probe_plugin_component.dart'
    show PluginComponentImports, pluginComponentComponent;

/// Same generated WIT surface as the first compiled guest, with a per-instance
/// counter to prove that a reloaded plugin gets fresh Dart state.
final class StatefulEchoPlugin implements wit.Plugin {
  StatefulEchoPlugin(this._engineService);

  final wit.EngineService _engineService;
  int _runCount = 0;

  @override
  Result<String, wit.PluginPluginError> run({
    required Borrowed<wit.ServiceTypesService> service,
    required String input,
  }) {
    final runCount = ++_runCount;
    final request = 'session.read-context';
    return switch (_engineService.call(service: service, request: request)) {
      OkResult(value: final value) => Result.ok('$value;run=$runCount'),
      ErrorResult(value: final error) =>
        Result.error(wit.PluginPluginError.service(error)),
    };
  }

  @override
  ({String id, String description}) describe() => (
    id: 'example/stateful-echo',
    description: 'Calls the granted engine service and tracks plugin-local runs.',
  );
}

void main() {
  pluginComponentComponent((PluginComponentImports imports) =>
      StatefulEchoPlugin(imports.pluginProbeEngineService));
}
