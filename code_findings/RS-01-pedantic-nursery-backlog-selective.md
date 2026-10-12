# RS-01 · 50 · Rust · Simplification · pedantic/nursery backlog (selective)

**Status: not implemented — awaiting your pick.** (Score 50 · Risk M · ~LOC Δ ~−80)

**Location:** workspace-wide; histogram in `/tmp/pedantic.log`
- **Finding:** `clippy -W pedantic -W nursery` reports ~175 pedantic + ~791 nursery hints. High-confidence, low-effort subset: 42×`missing_const_for_fn`, 14×`option_if_let_else`, 12×`assert_is_empty`, 10×`needless_collect`, 9×`or_fun_call`, 9×`iter_on_single_items` (~96 sites). Wholesale adoption is NOT proposed — repo policy enables a lint family only with every site fixed in one change.
- **Change:** one PR fixing that subset; do not flip the workspace lint set.
- **LOC Δ:** ~−80 · **Risk:** M (`option_if_let_else` restructures control flow)
- **Evidence:** pedantic histogram (counts above); lint names grep'd from the log.

---

*Saved from the Rust 1.99.0 bump + code-reduction review, 2026-10-12. Evidence is verbatim from the review; implement one ID at a time, re-running tests + clippy + ruff after each.*
