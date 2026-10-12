# Borrow shortlist: validated for implementation

Validated 2026-10-12 against the borrow sources (`~/projects/src/{pi,pi-fabric,codex}`) and prime-agent at `46e645655` plus the working tree. Every anchor below was opened and read in this pass.
Focus: **context efficiency, output quality, agent orchestration, overall effectiveness.**

**Ground rules for every item (AGENTS.md):** one PR per item; a red-first regression test; adoption telemetry in the same PR for anything user-visible (primitives only, fire-and-forget); a parity section in the PR body. The RLM kernel API and model-visible text are a **surface contract**, so anything marked *Rust-only divergence* needs maintainer sign-off (or the TS product adopts it too).

---

## Tier 1: internal changes, no surface change (implement now)

### 1. One-off auxiliary requests stop writing prompt-cache entries
*Source: pi `packages/coding-agent/src/core/compaction/compaction.ts:627-631` · Parity: **matches pi***

**Verified gap:** three one-off request builders use `StreamOptions { …, ..Default::default() }`, so `cache_retention` is `None` and *resolves* to Short (or Long under `PI_CACHE_RETENTION=long`, a 2× cache write):
- `crates/pa-core/src/session_engine/compaction_exec.rs:157-162` (compaction summarizer, 80–117 KB requests)
- `crates/pa-core/src/session_engine/refine.rs:602-605` (refine gate, ~41 KB per compaction)
- `crates/pa-core/src/session_engine/branch_summarization.rs:406-411` (branch summary; pi applies the opt-out here too)

`CacheRetention::None` already exists (`pa-types/src/ai/mod.rs:159`) and providers honor it (`anthropic/mod.rs:212`, `openai_completions/stream.rs:564`).

**Implement:** set `cache_retention: Some(CacheRetention::None)` at the three sites. For `session_id`, match pi: reuse the caller's id if one is passed (none is today), otherwise use a fresh uuid. Leave `decision_api.rs` alone (it isn't a one-off summary).

**Red-first test:** capture the summarizer's wire request with the existing faux/mock provider. Assert the Anthropic body has no `cache_control` anywhere, and that an openai-completions body has no `prompt_cache_key`. It fails today because the system block carries `cache_control`.

**Expected outcome:** aux requests no longer pay cache writes (1.25× or 2× input) and don't pollute the session's cache routing. Main-turn caching is unchanged.

---

### 2. Fall back to the session model when the auxiliary model fails at request time
*Source: codex `codex-rs/core/src/compact_model_fallback.rs` · Parity: n/a (`auxiliaryModel` is Rust-only)*

**Verified gap:** `resolve_auxiliary_model` (`auxiliary_model.rs:53-95`) only falls back at *resolution* time (unknown selector, not allowed, window too small). When the routed aux model **errors on the wire** (5xx, auth, overload), the compaction fails. `compact_session.rs` and `compaction_exec.rs` have no retry or fallback. This makes routing summaries to a cheap model (findings T05) unsafe today.

**Implement:** in `compact_session.rs` (history/turn-prefix calls) and `branch_summarization.rs`: if the routed model ≠ the session model and the call fails with anything other than an abort, retry **once** on the session model with the session key and headers. Like codex `should_retry_with_current_model`, never retry on abort or interrupt. Log the fallback as `warn_fallback` does (selector only, never error details).

**Red-first test:** aux model → mock that returns 500, session model → mock that succeeds. Assert compaction succeeds and its summary came from the session model. Also assert that an abort does *not* fall back.

**Expected outcome:** a broken cheap aux model degrades to the session model instead of failing compaction, so `auxiliaryModel` can safely point at a cheap model.

**Telemetry:** `compaction_model_fallback {purpose, outcome}`.

---

### 3. Cache-miss attribution telemetry
*Source: pi `cache-stats.ts` (TTL-gap vs ≤1,024-token noise) + codex `client.rs:385-448` (named discontinuity) · Parity: internal (telemetry only)*

**Verified anchor:** per-run `cache_read_tokens`/`cache_write_tokens` already exist (`pa-telemetry/src/catalog.rs:565-566, 674-675`).

**Implement:** at the turn boundary, classify each request whose cache read dropped versus the previous request into `ttl_gap` (idle > retention TTL), `prefix_changed` (model / tools / system-prompt hash changed; compare hashes only, never content) or `noise` (≤ 1,024 tokens). Emit it as a primitive `cache_miss_reason` property.

**Red-first test:** a pure classifier over synthetic usage + timestamp + hash sequences (whole-value asserts).

**Expected outcome:** cache spend becomes explainable, and items 1 and 8 get a before/after measure. The user-visible notice (pi `showCacheMissNotices`) is a separate, later TS-parity item.

---

## Tier 2: model-visible, *Rust-only divergence* (validated, needs sign-off)

### 4. Repeat-guard for identical `ipython` cells
*Source: pi-fabric `src/repeat-guard.ts` (33 LOC; warn 3 / block 6; added after a 220× `bash echo noop` loop on glm-5.3-flash)*

**Verified gap:** no loop detection anywhere in `pa-core`/`pa-daemon` (findings T09). prime-agent's single `ipython` tool maps 1:1 onto fabric's `fabric_exec`.

**Implement:** the tool is built once per session in `create_ipython_tool_definition` (`crates/pa-core/src/tools/ipython.rs:510`). Keep `Arc<Mutex<{last_code, count}>>` in that closure. The count compares exact code strings, and differing code resets it. At ≥3, append fabric's warning line to the result. At ≥6, don't execute; return an error result with fabric's block text. Put it behind a setting (`repeatGuard.enabled`, thresholds) so parity mode can switch it off.

**Red-first test:** run the definition with the existing `SettledCell` fake executor (`ipython.rs:560`): the 3rd identical call warns, the 6th is blocked (the executor is not called), and changed code resets the count.

**Expected outcome:** weak models can't burn a session repeating the same cell.

**Telemetry:** `repeat_guard_triggered {action: warn|block, count}`.

---

### 5. Tell the model what a failed cell already changed
*Source: pi-fabric `src/failure-progress.ts` (≤8 calls, paths ≤100 chars; "Successful calls may already have changed the workspace; inspect before repeating mutations.")*

**Verified gap:** `ExecuteResult` already records `diffs` (files edited) and `bash_commands` per cell (`ipython.rs:46-66`), but they go only into `details` (UI). The model's text is `format_execute_text` (`ipython.rs:244-268`): stdout/stderr/result/traceback. `edit()` *returns* "Edited …" without printing it (`skills/edit/src/edit/__init__.py:87`), so when a cell fails after some edits, **the model can't see which edits were applied**.

**Implement:** in `execute_ipython` (`ipython.rs:~424`), when `status == Error` and `diffs` or `bash_commands` is non-empty, append a bounded note: up to 8 edited paths (each ≤100 chars, `+N more`), the bash command count, and fabric's inspect-before-repeating line. Successful cells stay unchanged. No new kernel tracking is needed.

**Red-first test:** a `SettledCell` result with `status: Error` + 2 diffs gives text ending in the exact note (whole-string compare). Status `Ok` with diffs gives no note.

**Expected outcome:** fewer double-applied or blindly retried edits after a mid-cell failure.

---

### 6. A failure-aware output budget for cells (keep the end, spill the rest)
*Source: pi-fabric `src/output-budget.ts` (failures capped at 20,000 chars, middle truncation, full output in a 0600 artifact with `originalChars`/`omittedChars`)*

**Verified gap (bigger than the survey said):** cell output is capped at `DEFAULT_MAX_OUTPUT_CHARS = 65_536` **per stream** (`kernel/shared.rs:12`, applied in `kernel/manager/requests.rs:153` and `execution.rs:175-205`). The cap keeps the **head** and drops the tail. So one failing cell can carry about 196k chars (stdout + stderr + result), and a long failing test log loses its end, which is the useful part.

**Implement (two parts):**
1. Kernel buffering (`kernel/manager/events.rs` `append_truncated`): keep head + tail instead of head only, and save the full stream to a file in the session dir when it overflows.
2. Tool result (`ipython.rs` after `format_execute_text`): on `Error`, cap the combined text at 20k with middle truncation plus `[Full output (N chars) saved to: <path>]`. Successful cells keep today's budget.

**Red-first tests:** (a) a stream over the cap keeps its last line; (b) a failing cell with 100k chars of output yields ≤ 20k chars of text that ends with the artifact path, and the artifact holds the full output.

**Expected outcome:** failure context is smaller and keeps the informative end of the log.

**Size:** medium (it touches the kernel layer).

---

### 7. Carry list: `compact.carry(note)` / `compact.run(preserve=[…])`
*Source: pi-fabric `src/compaction/carry.ts` (bounded list stored as a session-log custom entry, replay-safe, malformed entry fails closed) · Plan: `next-steps/04-pr5-carry-and-preserve.md` (already accepted as new REPL surface)*

**Verified anchors:** `skills/compact/src/compact/__init__.py` (`status`, `run` only); `register_compact_handlers` (`session_engine/turn_boundary.rs:213`); `STATE_VERSION = 1` (`structural_compaction/state.rs:15`).

**Implement:** follow `next-steps/04`, with fabric's design for storage: a custom session row holds the current list (latest row on the branch wins, so reload and restart replay it). The deterministic State mirrors it (bump `STATE_VERSION`) and renders a protected "[Carried notes]" block. For the LLM engine, append the notes to the custom instructions. Bound the count and size.

**Tests:** as in next-steps/04. Notes survive 2 compactions + a worker restart (`pa-daemon/tests/deterministic_compaction_e2e.rs`); `preserve` appears in exactly one summary; the byte budget holds (extend the proptest).

**Expected outcome:** goals and constraints the user cares about survive every compaction. This is the direct fix for "sessions get dumb after compaction".

**Telemetry:** `compact_carry_used`.

---

## Tier 3: validated, needs a design pass first

### 8. Cache warmer + per-model prompt-cache TTLs
*Source: pi `core/cache-warmer.ts` (refresh at min(0.9·TTL, TTL−10 s); only when expected saving ≥ $0.05; idle-continuation probability 0.15; one-token replay guarded by `isReplayable`) + `ModelPromptCache` (`ai/src/types.ts:848`) · Parity: pi behavior*

**Verified gap:** no prompt-cache TTL field in `pa-types`/`models_generated.rs` (only `supports_long_cache_retention`, `compat.rs:42`).

**Design pass:** the catalog field must mirror the TS `models.generated.ts` (hand-maintained; AGENTS.md), and the warmer must be a background worker with a hanging-sink red-first test (paint-latency rule). Add the `cacheWarming: off|streaming|idle` setting as in pi.

**Expected outcome:** prefixes survive think pauses longer than 5 minutes without moving the default TTL to 1h.

### 9. Before/after bench harness (tooling, schedule early)
*Source: pi-fabric `bench/` (`run-matrix.sh`, `run-deepswe-*.sh`, `analyze.py`)*

Paired runs per (task, config, repetition): reward from `verify.sh`, tokens (input/cached/output), cost, turns, tool calls, and read-pathology stats. It's not shipped code; it's the evidence that items 1–8 actually improve effectiveness. Build it before or alongside Tier 2.

---

## Orchestration: validated gaps, blocked on one RLM API decision

`rlm.spawn(prompt, *, name, model=None, thinking=None, kind=None)` (`prime-agent-runtime/src/rlm/__init__.py:179`) has no result schema, cost ceiling, cwd/worktree or write policy. Children only have wall-clock waits (`rlm_children/lifecycle.rs:395`); their tokens are summed for telemetry (`telemetry.rs:330`) but never capped. All three borrows extend the surface contract, so **decide them together with the TS side**, then implement:

| Borrow | Source (verified) | Gives |
|---|---|---|
| Schema-validated child results (`schema=`) | fabric `agents/result.ts:51` `validateAgentResult` (typebox, first 5 errors) | compact structured answers instead of ≤64 KiB free text (T17) |
| Tree cost ceiling | fabric `agents/budget-ledger.ts` (append-only JSONL ledger; best-effort, can overshoot; checked before each spawn) | protection against runaway subagent spend |
| Child write policy + worktrees | fabric `agents/write-guard.ts` (`readOnly`/`writableRoots`/`shell: deny`, fails closed) + `worktree-manager.ts`; codex `worktree/` crate (Rust, Apache-2.0) | safe parallel writers and read-only reviewer children |

---

## Checked and rejected

- **codex retained tool outputs:** misread by the survey. It keeps client-authored *named outputs without a call* through compaction; it doesn't evict stale results.
- **fabric child tool allowlist:** it filters optional host tools, but prime exposes only `ipython`. `write-guard` (above) is the real borrow.
- **Foreman pattern:** it needs fabric's Jev runtime. Design inspiration only.
- **Rewriting the kernel-API prompt text:** it changes the cache-stable static layer and model-visible text. Do it only together with TS.
- **Orphan-repair after compaction:** the audit (mockrun4) found tool-pair integrity already holds.
- **Extension system, mesh/actors, sandboxing, providers, TUI:** outside the focus areas.

## Recommended order

1 → 2 → 9 (bench) → 4 → 5 → 7 → 3 → 6 → 8 → orchestration decision.
