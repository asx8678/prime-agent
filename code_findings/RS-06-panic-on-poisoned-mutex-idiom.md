# RS-06 · 20 · Rust · Correctness (informational) · panic-on-poisoned-mutex idiom

**Status: not implemented — awaiting your pick.** (Score 20 · Risk L · ~LOC Δ 0)

**Location:** `crates/pa-daemon/src/supervisor/clients.rs:83,94,96,343,357…`, `socket.rs`, `lease.rs`, `registration.rs` (`.lock().unwrap()` clusters)
- **Finding:** std-mutex poisoning panics in connection bookkeeping. Conventional crash-on-poison; no bad-input panics found in sampled request paths (`journal.rs` hits are all `#[cfg(test)]`). `let _ =` clusters are benign: channel/shutdown fire-and-forget drops, TS-parity comment at `acp/daemon.rs:393–395` states it; `passivation.rs` hits are all test cleanup.
- **Change:** optional: align on `.expect("…")` wording; otherwise leave as-is.
- **LOC Δ:** 0 · **Risk:** L
- **Evidence:** sed/rg context samples.

---

*Saved from the Rust 1.99.0 bump + code-reduction review, 2026-10-12. Evidence is verbatim from the review; implement one ID at a time, re-running tests + clippy + ruff after each.*
