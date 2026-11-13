#!/bin/sh
# Boot-phase measurement for the packaged host (issue #85 startup work).
#
# Usage: boot_phase_measure.sh <host-binary> <label> [runs]
#
# Drives the HEADLESS leg exactly like measure_startup.sh does (a single
# shutdown request over the pipes protocol, the same preopen shape as the
# stage-5 exit test) with TINA_WASM_BOOT_TRACE=1, and prints per-run phase
# lines plus the external wall clock to full exit. The host's trace goes
# to stderr; the phase table is the union of both streams.
#
# Methodology notes (kept identical between before/after runs):
# - identity-wrapped: the binary under test is invoked by absolute path
#   with the same flags every run; no timeout is changed anywhere;
# - wall clock = date +%s%N around the whole process (exec to exit);
# - first-ready = the trace's `guest:ready` mark (host-side timestamp of
#   the guest's first `{"type":"ready"}` stdout write);
# - TINA_MODELS_CACHE=1 adds a synthetic ~1 MB models.dev cache under the
#   config preopen (measures the guest's models-cache parse separately).
set -u
HOST_BIN="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
LABEL="$2"
RUNS="${3:-3}"
CFG=/tmp/boot-meas/cfg
WS=/tmp/boot-meas/ws
MODELS="${TINA_MODELS_CACHE:-0}"

mkdir -p "$WS" "$CFG"
if [ ! -f "$CFG/config" ]; then
  cat > "$CFG/config" <<'EOF'
version = 1

[default]
model = "anthropic/claude-sonnet-4-6"
EOF
fi
if [ "$MODELS" = "1" ] && [ ! -f "$CFG/.tina/cache/models_dev/models.json" ]; then
  echo "boot_phase_measure: synthetic models cache missing; generate it first (see boot_report notes)" >&2
  exit 1
fi
if [ "$MODELS" != "1" ] && [ -e "$CFG/.tina" ]; then
  rm -rf "$CFG/.tina"
fi

printf '{"op":"shutdown"}\n' > /tmp/boot-meas/in

i=1
while [ "$i" -le "$RUNS" ]; do
  rm -f /tmp/boot-meas/out /tmp/boot-meas/trace
  start=$(date +%s%N)
  TINA_WASM_BOOT_TRACE=1 \
    "$HOST_BIN" --headless \
    --preopen=.="$WS" --preopen=config="$CFG" \
    --env=TINA_LLM_ENDPOINT=http://127.0.0.1:8741 \
    --env=TINA_LLM_TOKEN=x \
    < /tmp/boot-meas/in > /tmp/boot-meas/out 2> /tmp/boot-meas/trace
  rc=$?
  end=$(date +%s%N)
  echo "--- $LABEL run $i (rc=$rc, models-cache=$MODELS)"
  echo "wall: $(( (end - start) / 1000000 ))ms exec-to-exit"
  cat /tmp/boot-meas/trace
  i=$(( i + 1 ))
done
