# Implementation plan — from the validated shortlist

2026-10-12. Scope verified this pass: every anchor in [shortlist.md](shortlist.md) re-opened in the tree; two of its corrections to the survey accepted and recorded (#7 codex retained-outputs misread; #8 child-allowlist → `write-guard.ts`). This plan turns the shortlist into PR-by-PR scope. It does not duplicate the shortlist's rationale — read that first.

**Standing gates for every PR** (AGENTS.md): `cargo fmt --all --check`, `clippy --workspace --all-targets -D warnings`, `cargo test --workspace`; one logical change per PR; a red-first regression test; adoption telemetry in the same PR for anything model-visible; a parity section (Tier-2 items are Rust-only divergences — maintainer sign-off, or TS adopts too).

**Order** (the shortlist's): 1 → 2 → 9 → 4 → 5 → 7 → 3 → 6 → 8 → orchestration decision.

---

## M1 — Tier-1 quick wins (no surface change)

### PR-1 · Aux requests opt out of caching *(shortlist 1 · TS parity · S)*
- **Files:** `session_engine/compaction_exec.rs:157`, `session_engine/refine.rs:602`, `session_engine/branch_summarization.rs:406` — all three verified to build `StreamOptions { …, ..Default::default() }`, so `cache_retention` *resolves* (Short, or **Long → 2× cache write** under `PI_CACHE_RETENTION=long`).
- **Change:** `cache_retention: Some(CacheRetention::None)` (variant exists, `pa-types/src/ai/mod.rs:159`; honored at `anthropic/mod.rs:212`, `openai_completions/stream.rs:564`) + a fresh uuid `session_id` (reuse caller's if one exists, per `compaction.ts:627-631`). Leave `decision_api.rs` alone.
- **Tests (red-first):** faux-provider wire capture per builder — anthropic body has **no `cache_control` anywhere**; openai-completions body has **no `prompt_cache_key`**. Fails today (system block carries `cache_control`).
- **Estimate:** ~20 lines + tests.

### PR-2 · Aux-model wire fallback *(shortlist 2 · internal · M)*
- **Files:** `compact_session.rs` (history/turn-prefix arms), `branch_summarization.rs`, `auxiliary_model.rs` (expose the session-model resolution for the retry).
- **Change:** routed aux model ≠ session model and the call fails non-abort → retry **once** on the session model (session key + headers). Never on abort/interrupt. `warn_fallback`-shaped logging (selector only).
- **Tests:** faux 500-on-aux → compaction succeeds via session model; abort → **no** fallback.
- **Telemetry:** `compaction_model_fallback {purpose, outcome}`.

## M2 — the evidence base

### PR-3 · Bench harness *(shortlist 9 · tooling · M-L)*
- **New:** `scripts/bench/` — paired before/after runs per (task, config, rep); task corpus with the `verify.sh` named-checks convention; metrics from existing session usage rows (input/cached/output, cost via `cache_pricing`), turns, tool calls, and **read-pathology stats** (whole-file read share) from session entries; an analyze report. Offline by default (faux or configured provider).
- **Not shipped code** — it is the before/after evidence for PR-4…PR-9.

## M3 — Tier-2 model-visible fixes (Rust-only divergence — sign-off required)

### PR-4 · Repeat-guard *(shortlist 4 · S-M)*
- **Files:** `tools/ipython.rs` — guard state as `Arc<Mutex<{last_code, count}>>` inside `create_ipython_tool_definition`'s closure (`ipython.rs:509-515`, verified); settings: `repeatGuard.enabled` + thresholds.
- **Semantics:** exact-code compare; differing code resets; ≥3 warns (fabric's line appended to the result); ≥6 blocks (error result, executor not called).
- **Tests:** via the `SettledCell` fake executor (`ipython.rs:560-568`, verified): 3rd identical warns, 6th blocks without execution, change resets.
- **Telemetry:** `repeat_guard_triggered {action: warn|block, count}`.

### PR-5 · Failed-cell progress note *(shortlist 5 · S)*
- **Files:** `tools/ipython.rs` execute path — `ExecuteResult.diffs`/`bash_commands` already exist (`ipython.rs:46-66`, verified) but ride only in `details`; `edit()` doesn't print its edits.
- **Change:** on `status == Error` with non-empty diffs/commands, append a bounded note: ≤8 edited paths (≤100 ch each, `+N more`), the bash count, fabric's inspect-before-repeating line. Success unchanged.
- **Tests:** `SettledCell` Error+2 diffs → whole-string note match; Ok+diffs → no note.
- **Telemetry:** `cell_failure_note {paths, commands}`.

### PR-6 · Carry list *(shortlist 7 · next-steps/04 · M-L)*
- **Files:** `skills/compact` (new `carry`/`drop`/`preserve` API), `session_engine/turn_boundary.rs:213` (handler registration), a **custom session row** (the `AgentMessage::Custom` precedent — latest row on the branch wins, so reload/restart replay it), `structural_compaction/state.rs` (**`STATE_VERSION` 1→2** + carried notes in State) and `render.rs` (protected `[Carried notes]` block), the LLM arm (notes appended to custom instructions). Bounds per fabric (`MAX_PRESERVE_*`).
- **Tests:** next-steps/04's suite + `deterministic_compaction_e2e.rs` (survive 2 compactions + worker restart), preserve-in-exactly-one-summary, byte-budget proptest extension.
- **Telemetry:** `compact_carry_used`.

## M4 — measurement and budgets

### PR-7 · Cache-miss attribution *(shortlist 3 · S-M)*
- **Files:** turn-boundary classification over existing per-run `cache_read_tokens`/`cache_write_tokens` (`pa-telemetry/src/catalog.rs:565-566, 674-675`, verified); new primitive `cache_miss_reason` = `ttl_gap` (idle > retention TTL) | `prefix_changed` (model/tools/system **hash** changed — hashes only, never content) | `noise` (≤1,024 tokens).
- **Tests:** pure classifier, whole-value asserts over synthetic sequences.

### PR-8 · Failure-aware output budgets *(shortlist 6 · M — touches the kernel)*
- **Files:** `kernel/manager/events.rs` (`append_truncated`: head+tail instead of head-only + artifact spill on overflow), cap application at `kernel/manager/requests.rs:153` and `execution.rs:175-205` (`DEFAULT_MAX_OUTPUT_CHARS = 65_536` per stream, verified at `kernel/shared.rs:12`); `tools/ipython.rs` — on `Error`, cap combined text at 20k with middle truncation + `[Full output (N chars) saved to: <path>]`. Success keeps today's budget.
- **Tests:** (a) a stream over the cap keeps its **last** line; (b) a failing 100k-char cell yields ≤20k text ending with the artifact path; artifact holds the full output.

## M5 — design-gated

### PR-9 · Cache warmer + `ModelPromptCache` *(shortlist 8 · L — needs its design pass)*
- Catalog field mirroring the hand-maintained TS `models.generated.ts` (AGENTS.md rule); `cacheWarming: off|streaming|idle` setting; background worker (paint-latency rule: **hanging-sink red-first test**, never on a render path); refresh at min(0.9·TTL, TTL−10 s) only when expected saving ≥ $0.05; one-token replay guarded by an `isReplayable` check (Anthropic budget-keyed cache entries).

## M6 — blocked on one decision

**The RLM API trio** — all three extend `rlm.spawn(prompt, *, name, model, thinking, kind)` (verified: `prime-agent-runtime/src/rlm/__init__.py:177-183` has none of them; children's usage is emitted for attribution (`rlm_children` usage emit) and waits are wall-clock only (`lifecycle.rs:395`) — no cost cap exists). Decide **together with the TS side**, then implement as three PRs:
1. `schema=` — validated child results (fabric `agents/result.ts:51`, typebox, first-5 errors) — closes T17 properly.
2. Tree cost ceiling — append-only JSONL ledger, checked before each spawn, best-effort with a hard per-execution call cap.
3. Child write policy + worktrees — fabric `write-guard.ts` semantics verified: `FabricWritePolicy {readOnly, writableRoots, shell: "deny"|"unconfined"}` via env, **fails closed to read-only+shell-deny on malformed policy**; plus the codex `worktree/` crate (Apache-2.0) for the isolation mechanics.

---

## Scope-of-work verdict

Every shortlist item survived this pass's re-verification — no anchor was wrong, and the two corrections it made to the survey (#7, #8) are accepted and now on record in the README. Total: **9 PRs + 1 decision gate**; M1 is a day of work, M3's three divergences are each small but need sign-off, PR-6 is the biggest single lift (~400 lines, lands an accepted next-step), and nothing in the plan touches the TS-parity wire except PR-1, which *restores* parity.
