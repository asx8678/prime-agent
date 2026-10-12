# RS-07 · 15 · Rust · Dependencies · duplicate majors (transitive; not actionable)

**Status: not implemented — awaiting your pick.** (Score 15 · Risk n/a · ~LOC Δ 0)

**Location:** `Cargo.lock` tree
- **Finding:** duplicates: getrandom 0.2/0.3/0.4, rand 0.8/0.9, rustix 0.38/1.1, syn 2/3, winnow, unicode-width, webpki-roots, linux-raw-sys. All transitive (pa-core's direct `getrandom = "0.3"` is fine; `rsa 0.9` pulls the legacy rand-0.8 chain). **Zero unused direct dependencies** in any crate (manual per-dep usage scan).
- **Change:** none now; dedupe rides future upstream bumps (rsa → rand 0.9).
- **LOC Δ:** 0 · **Risk:** n/a
- **Evidence:** `cargo tree -d` output; manual machete scan (empty result).

---

*Saved from the Rust 1.99.0 bump + code-reduction review, 2026-10-12. Evidence is verbatim from the review; implement one ID at a time, re-running tests + clippy + ruff after each.*
