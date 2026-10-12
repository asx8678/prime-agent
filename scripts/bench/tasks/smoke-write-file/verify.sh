#!/usr/bin/env bash
# Verifier for smoke-write-file (the harness's offline end-to-end check).
# Checks are derived mechanically from the task prompt's acceptance criteria.
# Usage: verify.sh <workdir> <result.json>
set -u
WORKDIR="$1"
RESULT_JSON="$2"
cd "$WORKDIR"

declare -a NAMES=()
declare -a PASS=()
check() { NAMES+=("$1"); PASS+=("$2"); }

# 1. smoke.txt exists
if [[ -f "$WORKDIR/smoke.txt" ]]; then check file_exists 1; else check file_exists 0; fi

# 2. content is the exact marker line
if [[ "$(cat "$WORKDIR/smoke.txt" 2>/dev/null)" == "prime-agent bench smoke" ]]; then check content_exact 1; else check content_exact 0; fi

JOINED_NAMES="${NAMES[*]}"
JOINED_PASS="${PASS[*]}"
export JOINED_NAMES JOINED_PASS
python3 - "$RESULT_JSON" <<'PYEOF'
import json, os, sys
names = os.environ["JOINED_NAMES"].split()
passes = [int(x) for x in os.environ["JOINED_PASS"].split()]
n = len(passes)
binary = 1 if n > 0 and all(p == 1 for p in passes) else 0
partial = (sum(passes) / n) if n else 0.0
res = json.load(open(sys.argv[1]))
res.update({
    "reward_binary": binary,
    "reward_partial": float(binary) if binary else partial,
    "checks": dict(zip(names, passes)),
})
json.dump(res, open(sys.argv[1], "w"), indent=2)
print(json.dumps({"reward_binary": binary, "reward_partial": res["reward_partial"], "checks": res["checks"]}, indent=2))
PYEOF
