# RS-05 · 25 · Rust · Comments · misleading parity comment

**Status: not implemented — awaiting your pick.** (Score 25 · Risk L · ~LOC Δ 0)

**Location:** `crates/pa-core/src/tools/mod.rs:11,21–23`
- **Finding:** `render_utils` is described as "consumed by the TUI-side renderer (parity-ported here)" — but pa-tui cannot link pa-core and carries its own copy (RS-04); pa-core's copy exists for parity/golden tests and the facade export. The comment misleads a reader into looking for a pa-tui consumer.
- **Change:** reword to "parity port of pa-tui's decoder; exercised by the golden tests and re-exported by the facade".
- **LOC Δ:** 0 · **Risk:** L
- **Evidence:** `mod.rs` text + `pa-tui/Cargo.toml` + RS-04 evidence.

---

*Saved from the Rust 1.99.0 bump + code-reduction review, 2026-10-12. Evidence is verbatim from the review; implement one ID at a time, re-running tests + clippy + ruff after each.*
