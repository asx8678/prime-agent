#!/usr/bin/env bash
# Orchestrate a benchmark run: isolated agent dir, then one cell per
# (task, config, rep), serial (a model call's OAuth refresh or catalog
# fetch can race across concurrent agents; serial keeps runs comparable).
#
# Adapted from pi-fabric bench/run-matrix.sh for prime-agent: the agent dir
# override is PRIME_AGENT_CODING_AGENT_DIR and configs name a binary + flags
# (scripts/bench/configs/<name>.sh) so a run can pair two builds of
# prime-agent itself — the before/after evidence base for the context and
# quality changes this harness exists to measure.
#
# Usage:
#   run-matrix.sh [--run-id ID] [--tasks slug,slug] [--configs a,b] [--reps N]
set -u

BENCH="$(cd "$(dirname "$0")" && pwd)"
RUN_ID="run-$(date +%Y%m%d-%H%M%S)"
TASKS=""
CONFIGS="baseline"
REPS=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --run-id) RUN_ID="$2"; shift 2 ;;
    --tasks) TASKS="$2"; shift 2 ;;
    --configs) CONFIGS="$2"; shift 2 ;;
    --reps) REPS="$2"; shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

# --- isolated agent dir: auth only (offline faux cells need nothing) ---
AGENT_DIR="$BENCH/results/$RUN_ID/agent"
mkdir -p "$AGENT_DIR"
python3 - "$AGENT_DIR" <<'PYEOF'
import json, os, sys
dst = os.path.join(sys.argv[1], "auth.json")
src = os.path.expanduser("~/.prime/agent/auth.json")
if os.path.exists(src) and not os.path.exists(dst):
    try:
        json.load(open(src))  # sanity: copy only a well-formed auth store
        import shutil
        shutil.copyfile(src, dst)
        print("agent dir prepared with copied auth store")
    except (OSError, json.JSONDecodeError):
        print("agent dir prepared with no auth (offline/faux run)")
else:
    print("agent dir prepared with no auth (offline/faux run)")
PYEOF

# --- task list ---
if [[ -z "$TASKS" ]]; then
  TASKS=$(ls "$BENCH/tasks" | grep -v '^README' | paste -sd, -)
fi

MANIFEST="$BENCH/results/$RUN_ID/manifest.json"
mkdir -p "$BENCH/results/$RUN_ID"
echo "{\"run_id\": \"$RUN_ID\", \"tasks\": \"$TASKS\", \"configs\": \"$CONFIGS\", \"reps\": $REPS}" > "$MANIFEST"

IFS=',' read -ra TASK_ARR <<< "$TASKS"
IFS=',' read -ra CFG_ARR <<< "$CONFIGS"
for slug in "${TASK_ARR[@]}"; do
  for cfg in "${CFG_ARR[@]}"; do
    for ((rep = 0; rep < REPS; rep++)); do
      CELL="$BENCH/results/$RUN_ID/$cfg/$slug/rep$rep"
      echo "=== cell $slug / $cfg / rep$rep ==="
      "$BENCH/run-cell.sh" "$BENCH/tasks/$slug" "$cfg" "$rep" "$CELL" "$AGENT_DIR" \
        || echo "CELL FAILED: $slug $cfg rep$rep"
    done
  done
done

python3 "$BENCH/analyze.py" "$BENCH/results/$RUN_ID"
echo "run complete: $BENCH/results/$RUN_ID"
