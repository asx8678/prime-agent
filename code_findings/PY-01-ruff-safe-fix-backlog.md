# PY-01 · 75 · Python · Simplification · ruff safe-fix backlog

**Status: not implemented — awaiting your pick.** (Score 75 · Risk L · ~LOC Δ ~−20)

**Location:** `prime-agent-runtime/` (src + test; exact lines listed below)
- **Finding:** 76 ruff hits are auto-fixable with plain `--fix` (no `--unsafe-fixes`): 57×UP037 quoted annotations (e.g. `src/rlm/bash.py:60,62`; `requires-python = ">=3.11"` makes them unnecessary), 15×C408 `dict()` calls (`test/test_harness.py:794–801`), 4×UP017 `datetime.UTC`, 3×F401 unused imports (`test_factory.py:72,73`, `test_mcp.py:5`), 3×F541, 2×UP035, 2×UP015.
- **Change:** run `ruff check --select F,B,SIM,UP,PIE,RET,ARG,ERA,C4 --fix` on the runtime; review the F401s by hand (public names otherwise untouched).
- **LOC Δ:** ~−20 · **Risk:** L
- **Evidence:** `ruff check … --output-format concise`: "Found 296 errors. [*] 76 fixable with `--fix`"; per-line output captured above.

---

*Saved from the Rust 1.99.0 bump + code-reduction review, 2026-10-12. Evidence is verbatim from the review; implement one ID at a time, re-running tests + clippy + ruff after each.*
