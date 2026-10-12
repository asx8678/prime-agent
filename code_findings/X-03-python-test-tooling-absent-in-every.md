# X-03 · 20 · Cross · Build · Python test tooling absent in every environment here

**Status: not implemented — awaiting your pick.** (Score 20 · Risk L · ~LOC Δ 0)

**Location:** `prime-agent-runtime/.venv` (pytest missing), no system pytest; vulture/deptry/machete/udeps also missing
- **Finding:** the Python suite — including the unrun `test_spawn_schema.py` (13 cases) and `test_cost_ceiling.py` (5 cases) — cannot execute in this environment; coverage unmeasured.
- **Change:** one approved install (`pytest`, `pytest-cov` into the runtime venv) unlocks the suite + the Phase-2 coverage lead. Needs your approval per the rules.
- **LOC Δ:** 0 · **Risk:** L

---

*Saved from the Rust 1.99.0 bump + code-reduction review, 2026-10-12. Evidence is verbatim from the review; implement one ID at a time, re-running tests + clippy + ruff after each.*
