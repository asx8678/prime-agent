# PY-04 · 25 · Python · Comments/Tests · documented no-op expression

**Status: not implemented — awaiting your pick.** (Score 25 · Risk L · ~LOC Δ −1)

**Location:** `prime-agent-runtime/test/test_subagent_registry.py:110` (`h.pid  # released as a deliberate background handle`)
- **Finding:** B018 useless expression; the comment declares intent, but the expression does nothing (the handle is retained by `handles.append(h)` anyway). `test_winjob.py` ARG001 hits are ctypes/FFI signatures — correct as-is.
- **LOC Δ:** −1 · **Risk:** L

---

*Saved from the Rust 1.99.0 bump + code-reduction review, 2026-10-12. Evidence is verbatim from the review; implement one ID at a time, re-running tests + clippy + ruff after each.*
