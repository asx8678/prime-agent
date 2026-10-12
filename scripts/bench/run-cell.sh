#!/usr/bin/env bash
# Run one benchmark cell: fresh task checkout, one prime-agent run, verifier,
# metric extraction from the session JSONL.
#
# Adapted from pi-fabric bench/run-cell.sh (the DeepSWE cell layout) for
# prime-agent: the agent is `prime-agent -p` (headless print mode), the agent
# dir override is PRIME_AGENT_CODING_AGENT_DIR, offline runs use
# PRIME_AGENT_FAUX_SCRIPT, and metrics come from the session JSONL rows the
# engine already records (usage + cost per assistant row).
#
# Usage: run-cell.sh <task-dir> <config> <rep> <cell-out-dir> <agent-dir>
#   config: a name with scripts/bench/configs/<name>.sh, or "baseline"
#           (baseline = ./target/release/prime-agent, no extra flags)
set -u

TASK_DIR="$1"
CONFIG="$2"
REP="$3"
CELL="$4"
AGENT_DIR="$5"

BENCH="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$BENCH/../.." && pwd)"
TASK="$TASK_DIR/task.json"

read -r SLUG BASE_REF AGENT_TIMEOUT VERIFY_TIMEOUT HAS_REPO FAUX_SCRIPT \
  < <(python3 - "$TASK" <<'PYEOF'
import json, sys
spec = json.load(open(sys.argv[1]))
fields = [
    spec.get("slug"),
    spec.get("base_ref"),
    spec.get("agent_timeout_s", 900),
    spec.get("verify_timeout_s", 300),
    "repo" if spec.get("repo") else "-",
    spec.get("faux_script") or "-",
]
print(" ".join(str(f) for f in fields))
PYEOF
)

mkdir -p "$CELL/session-store" "$CELL/session" "$CELL/logs" "$CELL/artifacts"
WORKDIR="$CELL/workdir"

# --- task checkout: git repo at base ref, or a local copy ---
if [[ "$HAS_REPO" == "repo" ]]; then
  REPO_URL=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['repo'])" "$TASK")
  CACHE="$BENCH/.cache/$(basename "$REPO_URL" .git)"
  if [[ ! -d "$CACHE/.git" ]]; then
    git clone --quiet "$REPO_URL" "$CACHE" || { echo "clone failed: $REPO_URL" >&2; exit 2; }
  fi
  git -C "$CACHE" fetch --quiet origin >/dev/null 2>&1 || true
  git clone --quiet "$CACHE" "$WORKDIR"
  # Pin the task's base state on the default branch names so agents that
  # start "from main" begin at the right tree.
  git -C "$WORKDIR" branch -f master "$BASE_REF" 2>/dev/null || true
  git -C "$WORKDIR" branch -f main "$BASE_REF" 2>/dev/null || true
  git -C "$WORKDIR" checkout --quiet "$BASE_REF"
else
  # Local task: optional seed dir in the task (workdir/), else a bare cell dir.
  if [[ -d "$TASK_DIR/workdir" ]]; then
    cp -R "$TASK_DIR/workdir" "$WORKDIR"
  else
    mkdir -p "$WORKDIR"
  fi
fi

# --- config: which binary + which flags ---
PRIME_AGENT_BIN="$REPO_ROOT/target/release/prime-agent"
PRIME_AGENT_FLAGS=()
PRIME_AGENT_ENV=()
CFG_FILE="$BENCH/configs/$CONFIG.sh"
if [[ -f "$CFG_FILE" ]]; then
  # shellcheck disable=SC1090
  source "$CFG_FILE"
fi
if [[ ! -x "$PRIME_AGENT_BIN" ]]; then
  echo "agent binary not found or not executable: $PRIME_AGENT_BIN" >&2
  echo "(build it: cargo build --release --bin prime-agent, or set PRIME_AGENT_BIN in configs/$CONFIG.sh)" >&2
  exit 2
fi

# --- offline default: a scripted faux provider ---
if [[ "$FAUX_SCRIPT" != "-" ]]; then
  SCRIPT_PATH="$TASK_DIR/$FAUX_SCRIPT"
  if [[ ! -f "$SCRIPT_PATH" ]]; then
    echo "faux script not found: $SCRIPT_PATH" >&2
    exit 2
  fi
  # The -p seam takes inline JSON (PRIME_AGENT_FAUX_SCRIPT is the script
  # itself, not a path); the daemon-worker seam is the one taking a file path.
  PRIME_AGENT_ENV+=("PRIME_AGENT_FAUX_SCRIPT=$(cat "$SCRIPT_PATH")")
fi

# --- agent run with a portable watchdog timeout ---
cd "$WORKDIR"
START=$(python3 -c 'import time;print(time.time())')
PROMPT="$(cat "$TASK_DIR/prompt.txt")"
(
  export PRIME_AGENT_CODING_AGENT_DIR="$AGENT_DIR"
  for kv in "${PRIME_AGENT_ENV[@]:-}"; do
    if [[ -n "$kv" ]]; then
      export "${kv%%=*}=${kv#*=}"
    fi
  done
  "$PRIME_AGENT_BIN" -p \
    --session-dir "$CELL/session-store" \
    --cwd "$WORKDIR" \
    --no-skills --no-prompt-templates --no-themes --no-context-files \
    ${PRIME_AGENT_FLAGS[@]:+"${PRIME_AGENT_FLAGS[@]}"} \
    "$PROMPT" \
    >"$CELL/logs/agent.stdout.txt" 2>"$CELL/logs/agent.stderr.txt" &
  AGENT_PID=$!
  ( sleep "$AGENT_TIMEOUT"; kill -TERM $AGENT_PID 2>/dev/null; sleep 20; kill -KILL $AGENT_PID 2>/dev/null ) &
  WATCHDOG=$!
  wait $AGENT_PID; AGENT_EXIT=$?
  kill $WATCHDOG 2>/dev/null
  echo "$AGENT_EXIT" > "$CELL/agent-exit-code.txt"
)
END=$(python3 -c 'import time;print(time.time())')
WALL=$(python3 -c "print(round($END - $START, 1))")

# --- session artifacts ---
find "$CELL/session-store" -name '*.jsonl' -exec cp {} "$CELL/session/" \; 2>/dev/null || true

# --- patch artifact (git tasks: the diff the agent produced) ---
if [[ "$HAS_REPO" == "repo" && -d "$WORKDIR/.git" ]]; then
  git -C "$WORKDIR" add -A >/dev/null 2>&1
  git -C "$WORKDIR" diff --cached "$BASE_REF" \
    -- . ':(exclude)vendor/**' ':(exclude)**/node_modules/**' ':(exclude).verify-bin/**' ':(exclude).verify-out/**' \
    > "$CELL/artifacts/model.patch" 2>/dev/null || true
fi

# --- metrics from the session JSONL rows ---
PYTHONPATH="$BENCH${PYTHONPATH:+:$PYTHONPATH}" python3 - "$CELL" "$WALL" "$CONFIG" "$SLUG" "$REP" <<'PYEOF'
import json, os, sys
from bench_metrics import collect_metrics

cell, wall, config, slug, rep = sys.argv[1], float(sys.argv[2]), sys.argv[3], sys.argv[4], sys.argv[5]
metrics = collect_metrics(cell)
exit_code = None
exit_path = os.path.join(cell, "agent-exit-code.txt")
if os.path.exists(exit_path):
    exit_code = int(open(exit_path).read().strip())
patch_path = os.path.join(cell, "artifacts", "model.patch")
patch_bytes = os.path.getsize(patch_path) if os.path.exists(patch_path) else 0
result = {
    "config": config,
    "task": slug,
    "rep": rep,
    "agent_exit_code": exit_code,
    "agent_wall_s": wall,
    "patch_bytes": patch_bytes,
    **metrics.as_dict(),
}
json.dump(result, open(os.path.join(cell, "result.json"), "w"), indent=2)
PYEOF

# --- verifier (named checks; updates result.json with reward_binary/partial/checks) ---
if [[ -x "$TASK_DIR/verify.sh" ]]; then
  (
    cd "$WORKDIR"
    ( "$TASK_DIR/verify.sh" "$WORKDIR" "$CELL/result.json" >"$CELL/logs/verify.stdout.txt" 2>"$CELL/logs/verify.stderr.txt" &
      VPID=$!
      ( sleep "$VERIFY_TIMEOUT"; kill -TERM $VPID 2>/dev/null; sleep 10; kill -KILL $VPID 2>/dev/null ) &
      WD=$!
      wait $VPID 2>/dev/null
      kill $WD 2>/dev/null
    )
  )
fi
echo "cell done: $SLUG $CONFIG rep$REP -> $CELL"
cat "$CELL/result.json"
