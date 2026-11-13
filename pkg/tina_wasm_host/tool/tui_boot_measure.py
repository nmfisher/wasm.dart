#!/usr/bin/env python3
"""TUI-leg boot measurement for the packaged host (issue #85 startup work).

Spawns `tina-wasm-host` (default TUI leg) under a pty so RawMode::enable
finds a terminal, with TINA_WASM_BOOT_TRACE=1, and arms the host's own
watchdog (TINA_TUI_KILL_MS) to inject Ctrl-C after the measurement
window so the guest exits cleanly. Prints the phase trace plus the
external exec-to-exit wall clock.

Usage: tui_boot_measure.py <host-binary> <label> [runs]
"""
import os
import pty
import select
import subprocess
import sys
import time

host = os.path.abspath(sys.argv[1])
label = sys.argv[2]
runs = int(sys.argv[3]) if len(sys.argv) > 3 else 2

for i in range(1, runs + 1):
    env = dict(os.environ)
    env["TINA_WASM_BOOT_TRACE"] = "1"
    env["TINA_TUI_KILL_MS"] = env.get("TINA_TUI_KILL_MS", "9000")
    env["TERM"] = "xterm-256color"
    master, slave = pty.openpty()
    start = time.time()
    proc = subprocess.Popen(
        [host, "--preopen=.=/tmp/boot-meas/ws", "--preopen=config=/tmp/boot-meas/cfg",
         "--env=TINA_LLM_ENDPOINT=http://127.0.0.1:8741", "--env=TINA_LLM_TOKEN=x"],
        stdin=slave, stdout=slave, stderr=slave, env=env, close_fds=True,
    )
    os.close(slave)
    trace = b""
    while True:
        r, _, _ = select.select([master], [], [], 1.0)
        if not r:
            if proc.poll() is not None:
                break
            continue
        try:
            chunk = os.read(master, 65536)
        except OSError:
            break
        if not chunk:
            break
        trace += chunk
    proc.wait()
    end = time.time()
    os.close(master)
    print(f"--- {label} TUI run {i} (rc={proc.returncode})")
    print(f"wall: {int((end - start) * 1000)}ms exec-to-exit")
    for line in trace.decode("utf-8", "replace").splitlines():
        if "boot phase" in line:
            print(line)
    sys.stdout.flush()
