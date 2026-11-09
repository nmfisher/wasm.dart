#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <isolated-wasm.dart-checkout> <output-component.wasm>" >&2
  exit 2
fi
repo=$(cd "$1" && pwd)
fixture=$(cd "$(dirname "$0")" && pwd)
output_dir=$(mkdir -p "$(dirname "$2")" && cd "$(dirname "$2")" && pwd)
output="$output_dir/$(basename "$2")"
sdk=${DART_SDK:-/home/agent/tools/dart-sdk-src/sdk/out/ReleaseX64/dart-sdk}
if [[ ! -x "$sdk/bin/dart" || ! -x "$sdk/bin/dartaotruntime" ]]; then
  echo "pinned Dart 3.14 SDK not found at $sdk" >&2
  exit 1
fi
sdk_version=$("$sdk/bin/dart" --version 2>&1)
if [[ "$sdk_version" != *"3.14.0-edge.e7112ac2438cd4f5ac0c832c27ce75bb10b02420"* ]]; then
  echo "wrong Dart SDK: $sdk_version" >&2
  exit 1
fi
if [[ -d "$repo/pkg/wasm_tools/lib/src/wit_gen/precompiled" ]]; then
  echo "source regeneration requires the source-build hook; precompiled assets are present" >&2
  exit 1
fi
if [[ -e "$repo/target" || -L "$repo/target" ]]; then
  echo "run in an isolated checkout with no target path; output must remain private" >&2
  exit 1
fi
cargo_target=${ISSUE85_CARGO_TARGET:-}
if [[ -z "$cargo_target" || ! -d "$cargo_target" ]]; then
  echo "set ISSUE85_CARGO_TARGET to an existing private, warmed Cargo target directory" >&2
  exit 1
fi
cargo_target=$(cd "$cargo_target" && pwd)
runtime_helpers=${ISSUE85_RUNTIME_HELPERS:-"$repo/target/wasm32-unknown-unknown/release/runtime_helpers.wasm"}
if [[ ! -f "$runtime_helpers" ]]; then
  echo "set ISSUE85_RUNTIME_HELPERS to an existing runtime_helpers.wasm; do not rebuild it in this slice" >&2
  exit 1
fi
if [[ ! -f "$repo/.dart_tool/package_config.json" || ! -f "$repo/.dart_tool/package_graph.json" ]]; then
  (cd "$repo" && "$sdk/bin/dart" pub get --offline)
fi
work=$(mktemp -d /tmp/issue85-plugin-probe.XXXXXX)
mkdir -p "$work/package/bin" "$work/package/hook" "$work/package/wit" "$work/package/generated" "$work/package/lib/src/generated" "$work/package/.dart_tool"
tool_config="$repo/.dart_tool/issue85-tool-package-config.json"
ln -s "$cargo_target" "$repo/target"
helper_destination="$repo/pkg/wasm_tools/assets/runtime_helpers.wasm"
copied_helper=0
if [[ ! -f "$helper_destination" ]]; then
  cp "$runtime_helpers" "$helper_destination"
  copied_helper=1
fi
cleanup() {
  rm -f "$repo/target" "$tool_config"
  if [[ "$copied_helper" -eq 1 ]]; then rm -f "$helper_destination"; fi
}
trap cleanup EXIT
cp "$fixture/pubspec.yaml" "$work/package/pubspec.yaml"
cp "$fixture/hook/link.dart" "$work/package/hook/link.dart"
cp "$fixture/plugin.wit" "$work/package/wit/plugin.wit"
plugin_source=${ISSUE85_PLUGIN_SOURCE:-"$fixture/plugin_authoring_stateful.dart"}
if [[ ! -f "$plugin_source" ]]; then
  echo "plugin source not found: $plugin_source" >&2
  exit 1
fi
cp "$plugin_source" "$work/package/bin/plugin_authoring.dart"
python3 - "$repo/.dart_tool/package_config.json" "$tool_config" "$repo" <<'PY'
import json, sys
from pathlib import Path
from urllib.parse import unquote, urlparse
src, dst, repo_root = map(Path, sys.argv[1:])
config = json.loads(src.read_text())
base = src.parent
for package in config['packages']:
    uri = package['rootUri']
    parsed = urlparse(uri)
    trailing_slash = uri.endswith('/')
    if parsed.scheme == 'file':
        path = Path(unquote(parsed.path))
        if path == Path('/workspace') or Path('/workspace') in path.parents:
            path = repo_root / path.relative_to('/workspace')
    else:
        path = (base / uri).resolve()
    package['rootUri'] = path.as_uri() + ('/' if trailing_slash else '')
for name, expected in [('wasm_tools', repo_root / 'pkg/wasm_tools'), ('wasm_components', repo_root / 'pkg/wasm_components')]:
    entry = next(p for p in config['packages'] if p['name'] == name)
    resolved = Path(unquote(urlparse(entry['rootUri']).path)).resolve()
    if resolved != expected.resolve():
        raise SystemExit(f'{name} package does not resolve to private checkout: {entry["rootUri"]}')
Path(dst).write_text(json.dumps(config, indent=2) + '\n')
PY
python3 - "$tool_config" "$work/package/.dart_tool/package_config.json" "$work/package" "$repo/.dart_tool/package_graph.json" "$work/package/.dart_tool/package_graph.json" <<'PY'
import json, sys
from pathlib import Path
src, dst, package_root, graph_src, graph_dst = map(Path, sys.argv[1:])
config = json.loads(src.read_text())
config['packages'] = [p for p in config['packages'] if p['name'] != 'issue85_plugin_probe']
config['packages'].append({'name': 'issue85_plugin_probe', 'rootUri': package_root.as_uri() + '/', 'packageUri': 'lib/', 'languageVersion': '3.14'})
Path(dst).write_text(json.dumps(config, indent=2) + '\n')
graph = json.loads(graph_src.read_text())
if 'issue85_plugin_probe' not in graph['roots']:
    graph['roots'].append('issue85_plugin_probe')
graph['packages'].append({'name': 'issue85_plugin_probe', 'version': '0.0.0', 'dependencies': ['wasm_components'], 'devDependencies': ['wasm_tools']})
Path(graph_dst).write_text(json.dumps(graph, indent=2) + '\n')
PY
config="$work/package/.dart_tool/package_config.json"
export PATH=/home/agent/.cargo/bin:$PATH CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=1 TMPDIR="$work" CARGO_TARGET_DIR="$cargo_target"
(
  cd "$repo"
  nice -n10 "$sdk/bin/dart" --packages="$tool_config" run "$repo/pkg/wasm_tools/bin/wasm_tools.dart" witgen \
    --input "$work/package/wit/plugin.wit" --output "$work/package/generated" --world plugin-component
)
cp "$work/package/generated"/*.dart "$work/package/lib/src/generated/"
cp "$repo/.dart_tool/native_assets.yaml" "$work/package/.dart_tool/native_assets.yaml"
(
  cd "$repo"
  nice -n10 "$sdk/bin/dart" --packages="$config" run "$repo/pkg/wasm_tools/bin/wasm_tools.dart" compile \
    --output "$work/plugin.component.wasm" --hooks-include-dev-dependencies \
    "$work/package/bin/plugin_authoring.dart"
)
python3 "$fixture/stamp_abi.py" "$work/plugin.component.wasm" "$output"
printf 'scratch: %s\ncomponent: %s\nsha256: ' "$work" "$output"
sha256sum "$output" | cut -d' ' -f1
