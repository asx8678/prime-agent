# PY-03 · 60 · Python · Tests · weak assertions and dead locals in test files

**Status: not implemented — awaiting your pick.** (Score 60 · Risk L–M · ~LOC Δ ~−10)

**Location:** `test/test_factory.py:2532,3834,4918,7661,7746`, `test/test_repl.py:2586,3019`, `test_factory.py:3220,6347,8522`
- **Finding:** F841 unused locals in tests — e.g. `flat = " ".join(doc.split())` (2532), `status = await self.settle(result)` (3834), `resume = await rlm_module.rlm.factory.resume(result["run_id"])` (4918): computed values that may have been *intended* as assertions. B017: `test_repl.py:3019` `with self.assertRaises(Exception): dill.load(fh)` asserts a blind `Exception` (any error passes). RET503: three helpers end without explicit `return`.
- **Change:** read each intent; either delete the dead local (keeping any side-effecting call) or add the missing assertion; tighten B017 to the expected exception type.
- **LOC Δ:** ~−10 · **Risk:** L–M (assertion intent must be judged per site)
- **Evidence:** ruff F841/B017/RET503 lines quoted; `sed` context confirms.

---

*Saved from the Rust 1.99.0 bump + code-reduction review, 2026-10-12. Evidence is verbatim from the review; implement one ID at a time, re-running tests + clippy + ruff after each.*
