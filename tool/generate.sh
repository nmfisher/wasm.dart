#!/bin/sh
set -e

(cd pkg/wasi && ./generate.sh)
(cd integration_tests && dart run tool/generate.dart)
(cd pkg/tina_wasm_host && dart run wasm_tools witgen -i world.wit)
(cd pkg/wasm_tools/example/greeting && dart run wasm_tools witgen -i test.wit)