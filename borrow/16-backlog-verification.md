# Pass 16: backlog verification — anchors pinned, one PR voided

Sixteenth pass, 2026-10-11 — pass 15's Phase-1 PR backlog named plausible file targets; this pass verifies each against the actual tree, pinning exact insertion points. One backlog PR is voided by a sixth correction. Extends passes [15](15-api-teaching-and-phase-one-backlog.md) and [10](10-self-audit-and-alignment.md). Nothing here is implemented.

---

## Correction #6 — the mutation queue is already ported

`crates/pa-core/src/tools/file_mutation_queue.rs` exists — "serialize file mutation operations targeting the same file… after resolving symlinks" — with per-file tokio mutexes in a global registry, wired into the edit tool at `tools/edit.rs:235` (`with_file_mutation_queue`). Pass 12's claim ("the genuine gap is file-mutation-queue.ts") compared TS `core/tools/` against the *Python kernel* and missed that **Rust `pa-core/src/tools/` mirrors TS `core/tools/` nearly 1:1** (bash, edit, edit-diff, file-mutation-queue, output-accumulator, truncate, golden-replay). The tool layer is fully ported; **item 58 and backlog PR 5 are void**. Master index updated.

(The same listing shows `output_accumulator.rs` present too — pass 12's "overlaps" note becomes "ported".)

## Verified anchors for the remaining PRs

| PR | Anchor (verified) | What's there today |
|---|---|---|
| 1 — summarizer/gate no-cache | `session_engine/compaction_exec.rs:157-168` | `SimpleStreamOptions::from_base(StreamOptions { max_tokens, api_key, headers, ..Default::default() })` → `complete_simple`/`stream_simple`. **Sharpened finding:** `cache_retention` defaults to `None`, which *resolves* (Short, or **Long via `PI_CACHE_RETENTION=long`** — a 2× cache write) — the summarizer doesn't merely skip the opt-out, it can honor the session's long-retention setting on a one-off ~82-117 KB request. Fix: explicit `Some(CacheRetention::None)` + a fresh routing `session_id` in this one block. Gate side: `refine.rs:447` ("`/refine` request options (session layer)") |
| 2 — repeat-guard | `kernel/manager/execution.rs` (cell execution) + `requests.rs` | the host-side cell-submission path. Note: `kernel/manager/repair.rs` already exists — protocol-level fail-frame/child-repair, a *different layer* than fabric's action-arg repairs (pass 2 §B.6); don't conflate them when porting |
| 3 — failure output budgets | `tools/output_accumulator.rs:319` (`truncate_tail` call site) | bash output truncation where success/failure is known; the cell-error path rides kernel `error` events (`kernel/protocol.rs:398`) |
| 4 — orphan-repair | `session_engine/compact_session.rs:172-195` | the kept-tail assembly around `cut.first_kept_entry_index` / `boundary_start` — the exact seam where tool-pair repair slots in |
| 6 — failure-progress digest | `session_engine/auto_retry.rs:64` (`run_turn_with_auto_retry`), injection at the retry decision (:94-101) | the retry loop already classifies non-retryable failures; the bounded digest injects where the retry is about to resend |
| 7 — carry list | `session_engine/structural_compaction/state.rs` + `render.rs` (pass 2 §B.2 design) | the State fold is the natural carry slot; the session-log row type follows fabric's `COMPACTION_CARRY_ENTRY_TYPE` shape |
| 8 — cache-miss attribution | telemetry already carries `cache_read_tokens` per run (`pa-telemetry/src/catalog.rs:565`; asserted in `session_engine/telemetry/tests.rs:281`) | the classification (TTL-gap vs ≤1,024-token noise floor) layers onto the existing per-turn usage flow — no new plumbing, a derivation |

## Sequencing impact

Phase 1 is now **seven PRs**, all independent except 8 pairing with the warmer. PR 1 is even more clearly first: the `..Default::default()` finding upgrades it from "parity nicety" to a real cost bug (long-retention environments pay 2× cache writes on every summarization and refine-gate call). Everything's anchors are verified — the backlog is patch-ready.

---

Sixteen passes, 61 items, **6 corrections**. The folder's final shape: survey (02–08) → synthesis (09) → audit (10) → sizing (11) → safety (12) → grounding (13–14) → plan (15) → **verified plan (16)**.
