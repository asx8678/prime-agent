# X-01 · 55 · Cross · Tests · packaging e2e hard-requires objdump

**Status: not implemented — awaiting your pick.** (Score 55 · Risk L · ~LOC Δ +6)

**Location:** `crates/pa-cli/tests/packaged_layout_e2e.rs:550` (via `scripts/release/split_debug.py:88` → `assemble_artifacts.py:415`)
- **Finding:** `packaging_dry_run_produces_artifact` fails on any host without binutils `objdump` — the only red in the 1.99.0 suite run on this machine.
- **Change:** a readiness guard that skips with a stated reason when `objdump` is absent (repo rule: a disabled test states its reason), or a dev-docs note that the packaging e2e needs binutils. Behavior on CI unchanged.
- **LOC Δ:** +6 · **Risk:** L
- **Evidence:** `/tmp/test_full.log` traceback: `FileNotFoundError: [Errno 2] No such file or directory: 'objdump'`.

---

*Saved from the Rust 1.99.0 bump + code-reduction review, 2026-10-12. Evidence is verbatim from the review; implement one ID at a time, re-running tests + clippy + ruff after each.*
