# Option 2 productionized: host side of the full wasm TUI

Design + seam inventory for the productionization round (milestone report 1;
implementation follows orchestrator review). The full design — closure
analysis, guest composition root, failure channel, validation plan — lives in
the tina repo: `docs/option2-wasm-tui.md` (branch
`codex/issue85-wasm-tui`). This file records the host half.

## Goal

Bare `tina-wasm-host` runs the FULL tina TUI (the real `runApp` front end
compiled into the component) inside the sandbox; `--headless` (alias
`--pipes`) keeps the pipes engine for agents and scripts. The host owns the
terminal: raw mode before the stdin pump, byte transport, live
`TIOCGWINSZ` window-size authority, Drop-guard restoration on every exit
path, epoch machinery for parked guests (spike findings 1/2 wired).

## What already landed in the merge (fefb33b)

- Branch `codex/issue85-wasm-tui` = `codex/issue85-host-tui-spike`
  (`cbd5cad`) merged with `codex/issue85-capability-merger` (`377d88d`);
  conflicts in `main.rs` + `Cargo.toml` resolved by combining. Host crate
  54/54 + integration suites green on the merged tree.
- `main.rs`: default → TUI leg, `--headless`/`--pipes` → the engine leg
  exactly as the capability-merger tip has it; `--tui` accepted as the
  spike's explicit form. No component path arguments on either leg.
- `build.rs`: second embed slot, `TINA_WASM_HOST_TUI` →
  `OUT_DIR/tui.component.wasm`, same rerun-if-changed staleness fix the
  engine got (the macOS stale-embed lesson applied up front); empty
  placeholder without the env var, surfaced as a rebuild hint at startup.
- `tui.rs`: the spike's runner carries over as
  `tina_wasm_host::tui::run_bytes(bytes)` (component from the embed, not a
  path); the shared stdout/stdin sinks it used moved to the crate root's
  visibility (`runner.rs` `pub(crate)`).

## Implementation milestones (this round)

1. **Production rewire of `tui.rs`**: replace the spike's bare Store with
   the `run_guest` machinery — discovery snapshot preopens (config mount,
   models.dev cache, HOME injection), `HostState` with the
   `tina:http@1.1.0` / `tina:policy@1.0.0` / `tina:process@1.1.0` /
   `tina:plugin-host` host imports on the shared linker (the engine guest
   simply doesn't import `tina:term`), raw mode + stdin pump + tty host
   import layered on top.
2. **`tina:term` grows the crash channel** (spike finding 4):
   `crash.fail(message)` host import records the message, signals the
   parked `run` wait (select on a `tokio::sync::Notify`/watch), tears down
   the store, restores the terminal, and exits non-zero with
   `guest crashed: <message>`. Guest side wraps the app body in
   `runZonedGuarded` → `fail(message)` → WASI `exit` backstop. Epoch trap +
   Ctrl-C watchdog remain the hang/wedge backstops.
3. **Tests**: tty/crash host imports, failure-channel end-to-end (throwing
   guest leg → restored terminal, non-zero exit, no parking),
   TUI-guest boots-to-ready smoke, `--headless` dispatch unit tests, and
   the existing 54 host tests unchanged and green.

## Validation (with the orchestrator's tmux leg)

Interactive tmux: keys, ANSI rendering, resize via the guest's 250 ms
`get-size` poll (push-on-SIGWINCH documented as follow-up), `/plugins`
(in-process registry; plugin session open/close crosses `tina:plugin-host`),
clean restore on quit and epoch kill, native TUI as the parity reference;
`--headless` pipes exit test unchanged both legs.
