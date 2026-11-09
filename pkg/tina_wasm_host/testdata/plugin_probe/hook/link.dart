import 'dart:convert';
import 'dart:io';

import 'package:hooks/hooks.dart';
import 'package:wasm_tools/hooks.dart';

void main(List<String> args) => link(args, (input, output) async {
  if (!input.config.buildWasmComponent) return;
  final abi = input.packageRoot.resolve(
    'generated/tina_plugin_probe_plugin_component.json',
  );
  output.dependencies.add(abi);
  output.assets.webAssemblyComponents.add(
    WasmComponentAsset(
      encoded: json.decode(
        File(abi.toFilePath()).readAsStringSync(),
      ) as Map<String, Object?>,
    ),
  );
});
