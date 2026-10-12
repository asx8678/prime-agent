# RS-08 · 15 · Rust · Tests (informational) · allow-list audit clean

**Status: not implemented — awaiting your pick.** (Score 15 · Risk L · ~LOC Δ 0)

**Location:** 134×`#[allow(dead_code)]`, 15×`#[allow(unused…)]`, 20×`#[ignore]` (all `#[ignore]` carry reasons: timing fixture, live kernel venv)
- **Finding:** spot-checks show documented parity/test-only reasons (tools/mod.rs modules, e2e harness fields). No bare ignores, no test-only modules without a stated reason.
- **Change:** none; recorded so the count is known to be deliberate.
- **LOC Δ:** 0 · **Risk:** L

---

*Saved from the Rust 1.99.0 bump + code-reduction review, 2026-10-12. Evidence is verbatim from the review; implement one ID at a time, re-running tests + clippy + ruff after each.*
