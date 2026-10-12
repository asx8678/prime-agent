# RS-04 · 30 · Rust · Duplication · image-dimension decoder exists twice (architecturally forced)

**Status: not implemented — awaiting your pick.** (Score 30 · Risk ? · ~LOC Δ 0 (−120 if ever consolidated))

**Location:** `crates/pa-core/src/tools/render_utils.rs:48–56` (+ facade re-export `crates/pa-core/src/lib.rs:36`) vs `crates/pa-tui/src/terminal_image.rs:130+`
- **Finding:** `get_image_dimensions_prefix` + `IMAGE_DIMENSIONS_PREFIX_BYTES` are implemented in both pa-core and pa-tui; pa-tui consumes its own copy because layering forbids pa-tui → pa-core (`pa-tui/Cargo.toml`: pa-types only).
- **Change:** none without an architectural PR (moving the source to pa-types is a new dependency edge + public-API move). The two files already cross-reference in doc comments. Recorded as accepted duplication.
- **LOC Δ:** 0 (−120 if ever consolidated) · **Risk:** H
- **Evidence:** both files; `pa-tui/Cargo.toml` dependency list.

---

*Saved from the Rust 1.99.0 bump + code-reduction review, 2026-10-12. Evidence is verbatim from the review; implement one ID at a time, re-running tests + clippy + ruff after each.*
