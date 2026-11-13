import 'package:wasm_components/wasm_components.dart'
    show Borrowed, Result;

import 'package:issue85_plugin_probe/src/generated/tina_plugin_probe.dart' as wit;
import 'package:issue85_plugin_probe/src/generated/tina_plugin_probe_plugin_component.dart'
    show PluginComponentImports, pluginComponentComponent;

/// Never returns from `run`: it exists to exercise epoch-deadline
/// cancellation and the trap/retire path of the plugin boundary. The
/// accumulating loop cannot be optimized away and its unbounded list growth
/// also exercises allocation during a live guest call.
final class SpinPlugin implements wit.Plugin {
  @override
  Result<String, wit.PluginPluginError> run({
    required Borrowed<wit.ServiceTypesService> service,
    required String input,
  }) {
    final sink = <int>[];
    var counter = 0;
    while (true) {
      sink.add(counter++);
    }
  }

  @override
  ({String id, String description}) describe() => (
    id: 'example/spin',
    description: 'Never returns; used to prove epoch-deadline cancellation.',
  );
}

void main() {
  pluginComponentComponent(
    (PluginComponentImports imports) => SpinPlugin(),
  );
}
