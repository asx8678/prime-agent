# X-02 · 25 · Cross · Build · repo-root session artifacts

**Status: not implemented — awaiting your pick.** (Score 25 · Risk L · ~LOC Δ n/a)

**Location:** `borrow/`, `next-steps/`, `validated_to_fix/`, `review-findings.md` (uncommitted; `findings/` already removed)
- **Finding:** working-tree audit artifacts at the repo root, not part of the build.
- **Change:** none proposed without your call — these were produced on your instruction; options: move out of repo, gitignore, or keep.
- **LOC Δ:** n/a · **Risk:** L

---

*Saved from the Rust 1.99.0 bump + code-reduction review, 2026-10-12. Evidence is verbatim from the review; implement one ID at a time, re-running tests + clippy + ruff after each.*
