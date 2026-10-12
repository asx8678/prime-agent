# prime-agent bench harness

Paired before/after evidence for the context-efficiency and quality changes
(the borrow shortlist's items): reward from per-task named-check verifiers,
tokens and recorded cost from the session JSONL the engine already writes,
turns, tool calls, and read-pathology stats (whole-file read share). Adapted
from pi-fabric's `bench/` (the DeepSWE cell layout) to prime-agent's surface.

This is tooling, not shipped code: its purpose is the before/after numbers for
changes like the repeat-guard, failure-aware output budgets, and compaction
improvements — a change lands with its paired run attached as evidence.

## Quick start (offline, no network, no auth)

```sh
# build the debug binary once, then run the smoke task through the full
# pipeline: scripted faux provider -> session JSONL -> metrics -> verifier -> analysis
cargo build --bin prime-agent
scripts/bench/run-matrix.sh --configs debug --tasks smoke-write-file
```

The smoke task proves the harness itself: a scripted faux model calls `bash`
to write a marker file, the verifier checks it, and `analyze.py` reports the
cell. Runs land in `scripts/bench/results/<run-id>/` (gitignored).

## Real runs

A real run pairs two builds of prime-agent — before and after a change —
under a real provider:

```sh
# build both
git checkout main && cargo build --release --bin prime-agent && cp target/release/prime-agent /tmp/pa-main
git checkout my-change && cargo build --release --bin prime-agent

# name the two binaries as configs
cat > scripts/bench/configs/before.sh <<'EOF'
export PRIME_AGENT_BIN=/tmp/pa-main
export PRIME_AGENT_FLAGS=()
EOF
cat > scripts/bench/configs/after.sh <<'EOF'
export PRIME_AGENT_BIN="$PWD/target/release/prime-agent"
export PRIME_AGENT_FLAGS=()
EOF

scripts/bench/run-matrix.sh --configs baseline,after --tasks smoke-write-file --reps 3
```

`baseline` is the working tree's release build by default (see
`configs/baseline.sh`). A config script is sourced by `run-cell.sh`; it sets:

- `PRIME_AGENT_BIN` — the `prime-agent` binary to run,
- `PRIME_AGENT_FLAGS` — extra CLI flags (an array), e.g.
  `export PRIME_AGENT_FLAGS=(--model anthropic/claude-opus-5-5 --thinking low)`.

The agent dir is isolated per run (`results/<run-id>/agent`, via
`PRIME_AGENT_CODING_AGENT_DIR`) with a copy of `~/.prime/agent/auth.json`
when it exists, so runs never touch the interactive agent state.

## Layout

- `run-matrix.sh` — orchestrates one run: isolated agent dir, one cell per
  (task, config, rep), serial; `analyze.py` at the end.
- `run-cell.sh` — one cell: fresh task checkout at `base_ref`, one headless
  `prime-agent -p` run under a watchdog timeout, session JSONL copy, patch
  artifact, metric extraction, verifier.
- `bench_metrics.py` — session-JSONL metric extraction (usage, recorded cost,
  tool calls, read pathology). Unit-tested in `test_bench_metrics.py`.
- `analyze.py` — per-config aggregates, per-task breakdowns, paired deltas
  with exact McNemar over solve flips. Unit-tested in `test_analyze.py`.
- `configs/<name>.sh` — a config names the binary + flags for its cells.
- `tasks/<slug>/` — the task corpus (see `tasks/README.md`).

## Metrics

From each cell's session JSONL (same layout as `~/.prime/agent/sessions`):

- **turns** — assistant rows; **tool calls** — `toolCall` blocks by name.
- **tokens** — `input`/`output`/`cacheRead`/`cacheWrite` sums per assistant row.
- **cost** — the recorded `usage.cost.total` sum: the engine already prices
  cache-aware costs per model (`pa-ai` `cache_pricing`), so no rate table is
  duplicated here.
- **read pathology** — the model-facing tool is `ipython` (the engine
  registers the kernel-backed tool itself); reads happen inside cell code:
  pathlib (`.read_text()`/`.read_bytes()`) and the kernel's `bash()` helper
  with shell readers (`cat`-family). Whole-file reads are unbounded reads:
  pathlib calls not sliced on the same statement, and `cat` (`head`/`tail`
  count as reads but are bounded).
- **results over 50 KB** — tool results whose text exceeds 50,000 chars.

## Offline default

A task with a `faux_script` in `task.json` runs fully offline through the
scripted faux provider (`PRIME_AGENT_FAUX_SCRIPT`, the same seam the CLI and
worker e2e harnesses use): deterministic inputs, no auth, no network. This
makes harness changes verifiable without provider spend.
