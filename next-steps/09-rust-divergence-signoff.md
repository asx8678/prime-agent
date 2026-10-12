# Rust-only divergences — sign-off brief (PR-1…PR-12 + next-steps/08)

Date: 2026-10-12. AGENTS.md's parity gate says: if Rust shows something TS
does not, that is a parity bug unless it is signed off. This record lists
every behavior the borrow plan (`borrow/implementation-plan.md`) and
`next-steps/08` added that the TS product does not have, with the evidence,
the opt-out, and a recommendation. **Nothing here is signed yet.** Each item
needs a maintainer decision (keep as a Rust-only divergence, change, or drop)
and, where it touches a shared surface, a TS answer (adopt, or accept the
divergence).

## How the TS side was checked

The TS checkout (`~/prime-agent`) is not on this machine, so the ground truth
is the pinned TS release CI already uses for lifecycle parity: **v0.9.8**
(`prime-agent-0.9.8.tgz`, `prime-agent-ai-0.9.8.tgz`,
`prime-agent-core-0.9.8.tgz` from the org repo's release, sha256-verified
against its `SHA256SUMS`). Only the published `dist/` sources were read;
nothing was executed. Paths below are inside those packages.

**Correction to earlier notes:** several change notes call their item a
"TS port" (`cache-warmer.ts`, `cache-stats.ts`, `completeSummarization`). Those
names come from upstream **pi** (and pi-fabric / codex), not from the Prime
Agent TS product: TS v0.9.8 has no `CacheWarmer`, no `cache-stats`, no
`completeSummarization`, and no `promptCache` catalog field. So PR-1, PR-7 and
PR-9 are Rust-only divergences too.

## Cache warming is opt-in (decided 2026-10-12)

`cacheWarming` now defaults to `off` (it was `streaming`). TS has no warmer
at all, and with the built-in Anthropic/OpenAI lifetimes warming really runs
and spends money, so it stays opt-in (`"cacheWarming": "streaming"` or
`"idle"` in global settings) until it is signed off. Whether to make it a
default again is decided together with the TS side.

## The items

| # | Item | What Rust does | TS v0.9.8 | Seen by | Opt-out |
|---|---|---|---|---|---|
| 1 | PR-1 one-off requests skip the prompt cache | compaction, branch-summary and refine calls send `cacheRetention: none` and a fresh routing id | `generateSummary` sends the **session** id with default retention, so it writes cache (`dist/core/compaction/compaction.js:439-454`) | provider bill, cache hit rate | none |
| 2 | PR-2 retry a failed auxiliary-model summary on the session model | one retry after a wire failure (not after abort), counted in telemetry | falls back only when the auxiliary model is **unusable at resolution** (`dist/core/agent-session.js:7717`); a wire failure fails the compaction | user (compactions that used to fail now succeed) | unset `auxiliaryModel` |
| 3 | PR-4 ipython repeat guard | warns at 3, blocks the 6th run of identical code with identical output | none | **model** | `repeatGuard.enabled: false` |
| 4 | PR-5 failed-cell progress note | failed cells that edited files or ran `bash()` list them | none | **model** | none |
| 5 | PR-6 `compact.carry` / `compact.drop` / `run(preserve=)` | new kernel API; `compaction_carry` session rows; structural state version 1→2 | none | **model**, session files | don't call it |
| 6 | PR-7 cache-miss attribution | 5 new counters on `agent session ended` | none | telemetry only (counts/hashes, never content) | telemetry opt-out |
| 7 | PR-8 failure-aware output budgets | failed cells capped at 20k chars, head + tail, full output saved to a linked temp file; over-cap streams keep their tail | head-only `[... output truncated at N chars ...]` (`dist/core/kernel/repl-manager.js:1107-1111`) | **model** | none |
| 8 | PR-9 prompt-cache warming | background one-token replays; opt-in (default `off`) | none | provider bill | default off |
| 9 | PR-10 `rlm.spawn(schema=…)` | collect validates the child's answer against a JSON Schema | none; TS ships the same Python `rlm` package (`dist/prime-agent-runtime/src/rlm/__init__.py`) without it | **model** (new API) | don't pass `schema` |
| 10 | PR-11 `rlm.set_cost_ceiling` | opt-in USD/spawn cap over the whole spawn tree, persisted; children inherit the remaining budget | none | **model** (new API), session files, daemon create metadata | opt-in |
| 11 | next-steps/08 idle passivation waits for a live warm window | a worker defers its own idle stop while warming (only when warming is opted in) | has `idleEvictionMinutes` but no warmer, so nothing to defer | worker lifecycle | default off (follows `cacheWarming`) |
| 12 | next-steps/05 context folding (`fold.begin` / `fold.end`) | gated `fold` skill; requests leave out the work between a closed fold's begin and end cells (the session log keeps it), add a reminder line to results inside an open fold, and compaction carries each fold's result; 5 telemetry counters | none | **model** (new API, gated), telemetry | default off (`contextFolding.enabled`) |

## Shared surfaces each item touches

- **Daemon wire (item 10).** A child's create command now carries
  `runtimeMetadata.rlmCostCeiling: {costUsd, spawnLimit}` when the parent has a
  ceiling. It is optional and additive. TS's runtime-metadata reader keeps only
  the keys it knows and drops the rest without error
  (`dist/package-manager-cli.js:625-655`), so a TS daemon or reader ignores it.
  A TS-hosted respawn would therefore drop the cap: an accepted degradation for
  an opt-in feature, but it needs the TS answer.
- **Session files (items 5, 10).** Rust writes `custom_message` rows of type
  `compaction_carry` and `rlm_cost_ceiling`. Rust never sends them to the model.
  **TS would:** its `convertToLlm` sends every custom row it does not list to
  the model as a user message (`dist/core/messages.js:446-451`). A Rust session
  opened in TS would show the carry list and ceiling rows to the model. Fix
  options: TS adds both types to its skip list, or Rust accepts the
  one-directional incompatibility.
- **Kernel API (items 5, 9, 10, 12).** `AGENTS.md` lists the RLM kernel API and the
  `compact` skill as a surface that must not change. `rlm.spawn(schema=)`,
  `rlm.set_cost_ceiling` and `compact.carry` / `compact.drop` /
  `compact.run(preserve=)` extend it, so each needs the TS answer from
  `next-steps/07-rlm-api-trio-decision.md`: adopt with the same names, or
  accept a Rust-only API. Calling `set_cost_ceiling` against a TS host fails
  loudly (unknown host request); `schema=` never reaches the host.
  The `fold` skill (item 12) is a gated skill package, not part of `rlm`:
  it loads only with `contextFolding.enabled`, and its `fold.*` host requests
  register only then. Folding writes no custom rows, so the TS skip-list
  hazard above does not apply; its markers ride the ipython result
  `details.contextFold`, which TS ignores, so a Rust session with folds
  opened in TS shows the full, unfolded history.
- **Telemetry schema (items 2, 6, 12, and the pre-existing repeat-guard,
  failure-note, output-budget, carry and warm counters).** New optional
  properties on `agent session ended`; counts only.

## Parity facts confirmed (no sign-off needed)

- TS sends the session id on session requests (`dist/core/agent-session.js:842`),
  as PR-9's routing-id change does.
- The TS faux provider counts a cache write on top of `input`
  (`prime-agent-ai` `dist/providers/faux.js:121-145`); the Rust faux matches,
  and the print-mode fixture was resized for it instead of changing the faux.

## Sign-off

For each item, record **keep (Rust-only)**, **change**, or **drop**, plus the
TS answer where a shared surface is listed.

| # | Maintainer | TS side | Notes |
|---|---|---|---|
| 1 | ☐ | n/a | |
| 2 | ☐ | n/a | |
| 3 | ☐ | n/a | |
| 4 | ☐ | n/a | |
| 5 | ☐ | ☐ API + skip list | |
| 6 | ☐ | ☐ schema | |
| 7 | ☐ | n/a | |
| 8 | ☐ (default now `off`) | n/a | |
| 9 | ☐ | ☐ API | |
| 10 | ☐ | ☐ API + `rlmCostCeiling` key + skip list | |
| 11 | ☐ | n/a | |
| 12 | ☐ (default off) | ☐ API + schema | evaluation (next-steps/05b step 8) decides any default |

Parity-diff evidence (frame diffs, interactive and wire comparisons against a
running TS binary) is still owed per item before merge. This brief covers the
source-level check only.
