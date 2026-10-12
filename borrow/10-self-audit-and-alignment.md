# Pass 10: self-audit of the borrow folder + alignment with next-steps

Tenth pass, 2026-10-11 — a different kind of deeper: instead of mining the sources further, this pass **audits the borrow folder against prime-agent itself** (~30 of its claims are "Rust lacks X"; each wrong one is a wasted port) and aligns the borrows with the repo's own `next-steps/` plans. One more correction falls out. Nothing here is implemented.

---

## W. Claim verification sweep

| Borrow claim | Verdict | Evidence |
|---|---|---|
| No cache warmer | **Holds** | the only `warm` hits are `WindowedSessionStore` session-file windowing (`session/window_tests.rs:181-188`) — warm reads of the JSONL window, not prompt-cache warming |
| No persistent memory subsystem | **Holds** | no `MemorySource`/memory-store matches; the harness-digest's `memory: 0` count is a placeholder with nothing behind it |
| No project trust | **Holds** | no `project_trust`/`trust_manager` anywhere — pairs with the path-traversal fix: `<cwd>/.prime/agent/settings.json` is honored ungated |
| No bug-report/crash-log | **Holds** | no matches |
| No classifier model support | **Holds** | the `classifier` hits in pa-ai are provider *failure classification* (stream-failure tests, vertex/bedrock event kinds), not classifier models |
| No SQLite | **Holds** | nothing in Cargo.lock; sessions are JSONL-only |
| No extension system | **Holds** | re-verified; only incidental string matches |
| No terminal image protocols | **Holds** | re-verified; hyperlinks only |
| No `pi-messages` provider | **Holds** | re-verified against the provider list |
| No Streamable HTTP MCP transport | **Holds, refined** | the MCP *client* lives in the Python kernel (`prime-agent-runtime/src/rlm/mcp.py`, `mcp_base.py`) with no `streamable` anywhere in the runtime; and pa-core/src/mcp is **larger than pass 2 assumed** — it carries OAuth flows (`oauth*`, `login`), `fetch_lane`, `remote_source`, `connection_store` — so the borrow narrows to the missing transport, not the module |
| No behavioral eval harness (item 19) | **Holds, refined** | prime-agent's Makefile "gates" exist but are **release-plumbing batteries** (changelog fold, restamp, CI-shard tooling, catalog assets, legacy upgrades, `cargo auditable` build) — they verify shipping machinery, not agent behavior. The TS `evals` borrow is a different category and stands |
| TUI polish pack incl. kill-ring (item 30) | **Corrections #4 and #5: the kill-ring AND fuzzy matching EXIST** | kill-ring: `crates/pa-tui/src/editor/mod.rs:14,24,102,162` (`mod kill_ring; pub use kill_ring::KillRing;`, fully wired). Fuzzy: `autocomplete/fuzzy_file_search.rs` plus agents-view search. Item 30 shrinks to **latex and oklab only** (both verified absent) |

Running correction count: **5** (queue modes ported — pass 3; ANSI stripped at context build — pass 6; jev listing artifact — pass 8; kill-ring exists — pass 10; fuzzy exists — pass 10, caught by the verification grep itself). Every negative claim in the folder is now either verified or corrected.

## X. Alignment with `next-steps/` — the repo's own plans the borrows accelerate

| next-steps plan | Aligned borrow | What the source adds |
|---|---|---|
| **02-pr4: the `history` skill** (`[u12]`/`[u12.c3]` address expansion, ~400 lines, bundled skill) | fabric `extractive-history` (pass 2 §B.2) | the design for *what* full text to surface behind an address: classifier-assisted verbatim evidence, off by default, consent-gated, tool-output/thinking excluded — plus the upstream's own warnings (no secret scanning, no cross-session retrieval) |
| **04-pr5: `compact.carry` + `compact.run(preserve=...)`** (~200 lines) | fabric `compaction/carry.ts` (item 9) | the replay-safety detail: the carry list lives as a **session-log custom entry** on the active branch (reload/restart/tree navigation replay it; no in-memory copy to lose) with `MAX_PRESERVE_*` bounds — exactly the `[Carried notes]` block the plan sketches |
| **05: context folding** (`fold.begin`/`fold.end`, gated skill, "ship behind a setting, default only if the evaluation shows a gain") | item 19 (bench with read-pathology stats) + extractive-history's bounds | the **evaluation** the plan itself demands is precisely the DeepSWE-style paired bench — and the plan's own caveat (the paper's gains came from RL-trained folding models) is the reason to keep the bench's reward-vs-token framing |
| **03-pr3: typed cell actions** | codex `compact_retained_tool_outputs` (item 5) + fabric typed truths | the retention/budget semantics for typed per-cell facts |

The roadmap in [09](09-pockets-and-roadmap.md) already sequences these: Phase 1 item 9 = next-steps/04; the Phase 4 evaluation harness = next-steps/05's gate. With this pass, every Phase-1/2 item maps to either an open audit finding or an already-planned repo PR — nothing in the early phases is speculative.

---

**Final state of the survey:** ten passes, 56 ranked items, 4 in-record corrections, all negative claims verified, and the early roadmap phases aligned with the repo's own plans. The borrow folder is now as honest as it is deep.
