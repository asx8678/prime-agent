# Task format

A task is a directory under `scripts/bench/tasks/<slug>/` with:

- `task.json` — required fields:
  - `slug` — the task name (directory name by convention),
  - `repo` + `base_ref` — a git repository and the commit the agent starts
    from; omit both for a local task (the cell's workdir is a fresh empty dir,
    or a copy of the task's `workdir/` seed dir when present),
  - `agent_timeout_s` (default 900) — watchdog timeout for the agent run,
  - `verify_timeout_s` (default 300) — watchdog timeout for the verifier,
  - `faux_script` — optional path (relative to the task dir) to a scripted
    faux-provider JSON, for offline tasks.
- `prompt.txt` — the prompt the agent receives.
- `verify.sh` — the verifier (see below).
- `workdir/` — optional seed directory copied as the working tree for local
  tasks.
- `faux.json` — the scripted provider responses (when `faux_script` names it).

## The verify.sh convention (named checks)

The verifier derives its checks mechanically from the task prompt's
acceptance criteria, accumulates them as *named checks*, and folds them into
`result.json` as `reward_binary` (all checks pass), `reward_partial` (mean of
passes), and `checks` (the per-name map). The convention (from pi-fabric's
bench) keeps every reward auditable by name — `analyze.py` pairs runs on
`reward_binary` and aggregates `reward_partial`.

```sh
#!/usr/bin/env bash
set -u
WORKDIR="$1"; RESULT_JSON="$2"; cd "$WORKDIR"
declare -a NAMES=(); declare -a PASS=()
check() { NAMES+=("$1"); PASS+=("$2"); }

# one check per acceptance criterion, named for what it proves:
if go build ./... ; then check build 1; else check build 0; fi
if go test ./...  ; then check tests 1; else check tests 0; fi

JOINED_NAMES="${NAMES[*]}"; JOINED_PASS="${PASS[*]}"
export JOINED_NAMES JOINED_PASS
python3 - "$RESULT_JSON" <<'PYEOF'
import json, os, sys
names = os.environ["JOINED_NAMES"].split()
passes = [int(x) for x in os.environ["JOINED_PASS"].split()]
n = len(passes)
binary = 1 if n > 0 and all(p == 1 for p in passes) else 0
partial = (sum(passes) / n) if n else 0.0
res = json.load(open(sys.argv[1]))
res.update({"reward_binary": binary, "reward_partial": float(binary) if binary else partial,
            "checks": dict(zip(names, passes))})
json.dump(res, open(sys.argv[1], "w"), indent=2)
print(json.dumps({"reward_binary": binary, "checks": res["checks"]}, indent=2))
PYEOF
```

Checks should observe *behavior*, not implementation: build the result, run
the tests, byte-compare outputs, grep for the promised artifact. Never
inspect the agent's transcript to decide reward.

## Adding a task

1. Pick a real user-visible behavior with mechanical acceptance criteria.
2. Write `prompt.txt` from the criteria (the agent gets no other context).
3. Write `verify.sh` from the same criteria, one named check each.
4. Pin the repo + `base_ref` (or provide a local `workdir/`).
5. Dry-run one cell: `scripts/bench/run-cell.sh scripts/bench/tasks/<slug> debug 0 /tmp/cell /tmp/agent`.
6. Add the task to a paired run: `run-matrix.sh --tasks <slug> --configs baseline,after --reps 3`.

The corpus is intentionally small; a task earns its place by measuring a
change the harness exists to evaluate (each shortlist item should name the
tasks its paired run uses).
