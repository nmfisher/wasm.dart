#!/usr/bin/env python3
"""Rebuild valid admission-negative component fixtures from plugin.component.wasm."""
import re
import subprocess
import sys
from pathlib import Path

fixture = Path(sys.argv[1]).resolve()
wasm_tools = sys.argv[2]
out = fixture / "admission-fixtures"
out.mkdir(parents=True, exist_ok=True)
text_file = out.parent / "plugin-base.component.wat"
with text_file.open("wb") as stream:
    subprocess.run([wasm_tools, "print", str(fixture / "plugin.component.wasm")], check=True, stdout=stream)
base = text_file.read_text()

def once(name: str, old: str, new: str) -> None:
    if base.count(old) != 1:
        raise SystemExit(f"{name}: expected one WAT fragment, found {base.count(old)}")
    (out / f"{name}.wat").write_text(base.replace(old, new))

once("wrong-engine-service-version", "engine-service@0.1.0", "engine-service@0.2.0")
member = base.replace('(export (;0;) "call" (func (type 7)))', '(export (;0;) "tick" (func (type 7)))')
if member == base or member.count('(export (;0;) "tick" (func (type 7)))') != 1:
    raise SystemExit("could not rename engine-service.call member")
old = '(alias export 1 "call" (func (;0;)))'
if member.count(old) != 1:
    raise SystemExit("could not find alias for engine-service.call")
(out / "wrong-engine-service-member.wat").write_text(member.replace(old, '(alias export 1 "tick" (func (;0;)))'))
once("wrong-plugin-export-version", "plugin@0.1.0", "plugin@0.2.0")
once("wrong-run-param", '(param "input" 30)', '(param "datum" 30)')
once("wrong-descriptor-field", '(field "description" 30)', '(field "summary" 30)')
once("wrong-run-resource-ownership", '(type (;35;) (borrow 3))', '(type (;35;) (own 3))')
trap, count = re.subn(
    r'    \(func \$_componentStart \(;2231;\) \(type 193\)\n.*?\n    \)',
    '    (func $_componentStart (;2231;) (type 193)\n      unreachable\n    )',
    base, count=1, flags=re.S,
)
if count != 1:
    raise SystemExit("could not find generated guest start function")
(out / "init-trap.wat").write_text(trap)
for wat in sorted(out.glob("*.wat")):
    binary = wat.with_suffix(".component.wasm")
    subprocess.run([wasm_tools, "parse", str(wat), "-o", str(binary)], check=True)
    subprocess.run([wasm_tools, "validate", str(binary)], check=True)
    wat.unlink()
text_file.unlink()
