import 'package:wasm_components/wasm_components.dart'
    show Borrowed, ErrorResult, OkResult, Result;

import 'package:issue85_plugin_probe/src/generated/tina_plugin_probe.dart' as wit;
import 'package:issue85_plugin_probe/src/generated/tina_plugin_probe_plugin_component.dart'
    show PluginComponentImports, pluginComponentComponent;

/// Dummy stateful proof plugin (issue #85, principal decision). One
/// artifact exercises the whole stateful contract on the current interface:
/// a per-instance call counter (state held across calls), the explicitly
/// granted `session.read-context` engine service, and a kill marker in the
/// session model that parks the synchronous `run` until the host's epoch
/// deadline interrupts the call — revoking the lease and discarding the
/// Store, so the next attach starts a fresh counter.
final class StatefulProofPlugin implements wit.Plugin {
  StatefulProofPlugin(this._engineService);

  /// The broker answers as JSON: `{"session_id":...,"model":...}`. Setting
  /// the model to `kill-me` parks the guest mid-call.
  static const killMarker = '"kill-me"';

  final wit.EngineService _engineService;
  int _runCount = 0;

  @override
  Result<String, wit.PluginPluginError> run({
    required Borrowed<wit.ServiceTypesService> service,
    required String input,
  }) {
    final runCount = ++_runCount;
    return switch (_engineService.call(
      service: service,
      request: 'session.read-context',
    )) {
      OkResult(value: final value) => _finish(value, runCount),
      ErrorResult(value: final error) =>
        Result.error(wit.PluginPluginError.service(error)),
    };
  }

  Result<String, wit.PluginPluginError> _finish(
    String context,
    int runCount,
  ) {
    if (context.contains(killMarker)) {
      // Guest code is synchronous; only the host's epoch deadline can
      // interrupt it. Hold the boundary until it fires.
      var sink = 0.0;
      while (true) {
        sink = sink * 1.0000001 + 1.0;
        if (sink.isInfinite) sink = 1.0;
      }
    }
    return Result.ok('$context;run=$runCount');
  }

  @override
  ({String id, String description}) describe() => (
    id: 'example/stateful-proof',
    description:
        'Dummy stateful proof: counter, granted service, epoch-kill marker.',
  );
}

void main() {
  pluginComponentComponent((PluginComponentImports imports) =>
      StatefulProofPlugin(imports.pluginProbeEngineService));
}
