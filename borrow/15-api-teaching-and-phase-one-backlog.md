# Pass 15: how fabric teaches its API — and the Phase-1 PR backlog

Fifteenth pass, 2026-10-11 — the survey is mined out, so this closes the arc: the model-facing API reference as a prompt-design exemplar (the angle the original token audit started from), the full fabric configuration surface, and the roadmap's Phase 1 converted into an execution-ready PR backlog under the repo's own rules. Nothing here is implemented.

---

## AL. The model-facing reference pattern — prompt design for API surfaces

`skillsets/typescript/fabric-exec/references/agents.md` is the exemplar of teaching an API inside a prompt. Its conventions, all applicable to prime-agent's own RLM/kernel API prompt sections:

- **Canonical naming with aliases stated once**: `agents.wait` is canonical, `join` its alias — same arguments, same behavior, one sentence.
- **Single options object per method** — every method takes one object; no positional soup.
- **Per-field failure semantics in-line**: `cwd` "is a filesystem path only… invalid or non-directory targets fail without falling back" — the no-silent-fallback rule is written where the field is described.
- **Outcome honesty**: `outcome: "indeterminate"` "is never retried automatically, so inspect before re-running."
- **Bounds that only narrow**: `thinkingBounds` "must lie inside the caller's bounds or the call fails, and children never widen them."
- **Resolution orders spelled out**: model resolution = aliases → exact → closest fuzzy (recency ties break toward recently used), and unresolved values "fail before launch."
- **Trust semantics per runner documented inline** (which runner adds no gate, which inherits saved decisions).
- **The pointer hierarchy in practice**: SKILL.md carries the contract; `agents.md`/`mesh.md`/`mcp.md` load as hard pointers on demand — the always-on surface stays one reference (the pass-9 skills contract).

The original audit measured the system prompt's *size*; this is the missing half — the *shape* of API-teaching text. Adopting these conventions in prime-agent's kernel API docs costs nothing and reads better to models.

## AM. The complete fabric configuration surface

`docs/configuration.md` — **40 sections**: execution kernels, full reference, Jev System One, lossless decision profiles, components, speculation, thinking bounds, prewalk executor, models, third-party providers, result formatting, code modes, captured extension tools, approvals and risk, **temporal retention**, agents, MCP, UI, mesh, memory sources. No new borrow items — it is the reference to consult for the settings shape of any adopted fabric-pattern feature.

---

## AN. Phase-1 PR backlog — execution-ready, per the repo's own rules

One logical change per PR; each carries a red-first regression test; adoption telemetry rides with anything user-visible; parity divergences flagged in the PR description.

| PR | Item | Files | Red-first test | Parity | Telemetry |
|---|---|---|---|---|---|
| 1 | §0 summarizer/gate `CacheRetention::None` + fresh routing ID | `compaction_exec.rs`, refine-gate request build | aux request carries `cache_retention: None` and a non-session routing id | exact TS behavior (`compaction.ts:622-631`) — none needed | internal fix, none |
| 2 | repeat-guard (8) | kernel cell-submission path (host side of submitted code) | 4th identical submission warns, 7th blocks; differing code resets | Rust-only, fabric-inspired — flag as divergence | `repeat_guard_warned` / `repeat_guard_blocked` |
| 3 | failure output budgets (34) | `tools/truncate.rs` + result render | failing tool result capped at 20k chars with artifact path + `omittedChars`; success keeps configured budget | Rust-only divergence | none |
| 4 | orphan-repair (3) | compaction message-validity layer | cut leaving an orphaned result drops it; unanswered call gets a synthetic result; clean input returns byte-identical (cache prefix untouched) | Rust-only | `compaction_orphan_repaired` count |
| 5 | per-file mutation queue (58) | host write/edit tool path | concurrent writes to the same realpath serialize; ENOENT falls back to resolved path | mirrors TS `file-mutation-queue.ts` | none |
| 6 | failure-progress digest (38) | session retry/nudge path | repeated failure injects a bounded 8-call digest; success injects nothing | Rust-only | `failure_progress_shown` |
| 7 | carry list (9 — lands `next-steps/04`) | `structural_compaction/` state+render, `compact.carry` kernel API | carry entry is a session-log row: survives reload, renders in every summary, bounds enforced, drop/replace works | `next-steps/04` API + fabric's session-log-entry design | `compact_carry_used` |
| 8 | cache-miss attribution + notices (10+22) | pa-telemetry event + session boundary | idle-gap > TTL labeled as TTL miss; ≤ 1,024-token misses are noise-floor, unlabeled | TS `cache-stats.ts` reference | `cache_miss_notice` (the user-visible event) |

Sequencing: PR 1 first (smallest, direct parity). PRs 2–6 are independent. PR 7 is the `next-steps/04` implementation and should reference the fabric design for the replay-safety detail. PR 8 pairs with the cache warmer (item 2) as its visibility layer but lands independently. The cache warmer itself is the one Phase-1 item needing its own design pass (paint-latency red-first test, `ModelPromptCache` catalog plumbing) — schedule it after PRs 1–6 prove the pattern.

---

**The borrow folder now spans survey → verification → sizing → roadmap → PR backlog.** Fifteen passes, 61 items, 5 corrections, and Phase 1 executable from the table above.
