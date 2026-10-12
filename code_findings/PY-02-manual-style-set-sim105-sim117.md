# PY-02 · 50 · Python · Simplification · manual style set (SIM105/SIM117)

**Status: not implemented — awaiting your pick.** (Score 50 · Risk L · ~LOC Δ ~−25)

**Location:** `src/rlm/bash.py` (×10 SIM105), `src/rlm/__init__.py:494`, `src/rlm/_winjob.py:375`, 89× SIM117 in tests
- **Finding:** `try/except: pass` blocks → `contextlib.suppress`; nested `with` → combined. Not auto-fixable; cosmetic.
- **LOC Δ:** ~−25 · **Risk:** L
- **Evidence:** ruff SIM105/SIM117 lines; total 296 hits (103 src / 193 test).

---

*Saved from the Rust 1.99.0 bump + code-reduction review, 2026-10-12. Evidence is verbatim from the review; implement one ID at a time, re-running tests + clippy + ruff after each.*
