# RS-03 · 35 · Rust · Simplification · `use_self` (135 sites)

**Status: not implemented — awaiting your pick.** (Score 35 · Risk L · ~LOC Δ 0)

**Location:** workspace-wide (`crates/*/src`)
- **Finding:** 135 nursery hits: impl blocks naming their own type instead of `Self`.
- **Change:** mechanical rewrite (could ride with RS-02 or RS-01). Pure cosmetics; review churn is the cost.
- **LOC Δ:** 0 · **Risk:** L
- **Evidence:** pedantic histogram: `clippy::use_self` 135.

---

*Saved from the Rust 1.99.0 bump + code-reduction review, 2026-10-12. Evidence is verbatim from the review; implement one ID at a time, re-running tests + clippy + ruff after each.*
