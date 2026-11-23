#!/bin/sh
set -e

(cd pkg/wasi && ./generate.sh)
(cd integration_tests && dart run tool/generate.dart)
(cd pkg/wasm_tools/example/greeting && dart run wasm_tools witgen -i test.wit)