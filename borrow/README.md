# Borrow survey: what to lift from `~/projects/src` into prime-agent

Surveyed 2026-10-11 against prime-agent @ `46e645655` (plus the Tier-1 fix working tree). Deep dives: [02](02-pi-and-fabric-deep-dive.md) · [03](03-extensions-agents-and-verification.md) · [04](04-tui-wire-and-state.md) · [05](05-loop-protocol-and-budgets.md) · [06](06-bash-ansi-workflows-and-grants.md) · [07](07-compat-certification-and-skills.md) · [08](08-substrate-chord-entropy-ux.md) · [09](09-pockets-and-roadmap.md) · [10](10-self-audit-and-alignment.md) · [11](11-testing-and-sizing.md) · [12](12-tools-safety-and-python-fabric.md) · [13](13-provider-protocol-and-memory-pipeline.md) · [14](14-design-closeout.md) · [15](15-api-teaching-and-phase-one-backlog.md) · [16](16-backlog-verification.md) · [17](17-bend-entropy-mesh-actors.md) · [18](18-context-and-caching.md) · [19](19-codex-deep-dive.md) · [20](20-codex-fit-map.md) — roadmap (v2) in [09](09-pockets-and-roadmap.md), master index below, Phase-1 PR backlog in [15](15-api-teaching-and-phase-one-backlog.md) with verified anchors in [16](16-backlog-verification.md), validated shortlist in [shortlist.md](shortlist.md), execution plan in [implementation-plan.md](implementation-plan.md). Sources (all local checkouts):

- **pi** — `/home/adam/projects/src/pi` @ `cdaf6c227` — the TS upstream (parity ground truth)
- **codex** — `/home/adam/projects/src/codex` @ `8c0f701167` — OpenAI Codex CLI, `codex-rs/` (127 crates, Rust)
- **pi-fabric** — `/home/adam/projects/src/pi-fabric` @ `9cea25d` (0.112.0) — the Fabric orchestration runtime

Cross-referenced against `findings/tokens-and-context/` (T-prefixed items) and `validated_to_fix/` (Tier-2 items). Nothing here is implemented.

---

## 0. New defect found while surveying (actionable now, small)

**One-off auxiliary requests write prompt-cache entries keyed to the session.**

TS's `completeSummarization` (`pi/packages/coding-agent/src/core/compaction/compaction.ts:622-631`) explicitly sets `cacheRetention: "none"` plus a fresh routing ID (uuidv7) for one-off summaries: *"Avoid cache writes for one-off summaries."* Prime-agent's summarizer and auto-refine gate do neither (verified: no `cache_retention` anywhere in `pa-core/src/session_engine/compaction_exec.rs`, `auxiliary_model.rs`, or `refinement/`), so every ~82–117 KB auxiliary request (`findings/tokens-and-context/05`) pays a cache-write and pollutes the session's cache key.

Fix: thread `CacheRetention::None` (+ a fresh `session_id`) through the summarizer and gate request options in pa-core. Direct TS parity, a few lines, no wire-shape change.

## 1. Tier-2 parity answers (the `pi` checkout settles the open decisions)

| Item | TS evidence | Consequence for prime-agent |
|---|---|---|
| **T01** two-block system prompt | `pi/packages/ai/src/api/anthropic-messages.ts:1184-1193` — non-OAuth path sends the system prompt as ONE cached block (OAuth adds a separate Claude-Code identity block) | The single-block shape IS parity. The finding-01 static/dynamic split stays worth doing, but must be flagged as a deliberate improvement beyond TS |
| **T02** cache TTL default | `anthropic-messages.ts:59-76` `resolveCacheRetention`: default `"short"`, `PI_CACHE_RETENTION=long` env, **no settings key** — identical to Rust | Don't change the default. Port the cache warmer instead (§2.1) — that is TS's actual answer |
| **T04** harness-digest | `harness-digest` does not exist anywhere in the TS tree | Rust-side invention; collapse/trim freely, zero parity risk |
| **T05** auxiliary model | `auxiliaryModel` does not exist in TS compaction | Rust-side invention; see codex's fallback ladder (§3.2) for the reference design |
| **A02** provider idle timeouts | `6b07b4e57` "apply Mistral request timeout only to response headers" | TS does apply timeouts; A02 is a real gap, not a parity choice |
| parity caveat | this checkout is a live TS tree | `validated_to_fix/README.md`'s "TS checkout does not exist on this machine" caveat can be lifted for every Tier-2 item |

## 2. Borrow from **pi** (TS upstream)

### 2.1 The cache warmer — the real T02 fix

`pi/packages/coding-agent/src/core/cache-warmer.ts` (~450 lines):

- refreshes the prompt-cache entry at **90% of TTL with a 10 s margin** (`getCacheWarmingDelayMs`);
- sends a refresh **only when the expected saving ≥ $0.05** (measured against cache-read vs cache-write pricing);
- models the chance a real request arrives while idle at **0.15** (IDLE_CONTINUATION_PROBABILITY, measured — per-session estimates were not better);
- replays the request with a **one-token output cap** when replayable; `isReplayable` guards Anthropic budget-keyed cache misses (budget_tokens derives from max_tokens on non-adaptive-thinking models);
- settings key exists: `cacheWarming: "off" | "streaming" | "idle"` (`pi/packages/coding-agent/src/core/settings-manager.ts:52,914-921`).

Requires borrowing **`ModelPromptCache`** (`pi/packages/ai/src/types.ts:848`) — per-model cache TTL tiers in the catalog (`model.promptCache[retention]` seconds). pa-types/pa-ai have no equivalent field today.

Fit: pa-models catalog field + pa-core background warmer (must respect the paint-latency rule in AGENTS.md: the warmer is a background worker, never on a render path; needs a red-first hanging-sink test). Effort: medium. Value: kills the "credits burn fast on think gaps" signature that motivated finding 02.

### 2.2 Port candidates from recent upstream commits

- `f44707e7f` durable: bound the Session's document cache by use (`documentRetentionMs`) — the same shape as our "old tool results never evicted" gap
- `6b07b4e57` Mistral header-only request timeout — the A02 reference implementation
- `ce950d78f` use models.dev metadata for the model catalog — catalog sourcing for pa-models
- `f1b2e77f5` load symlinked AGENTS.md in nested git worktrees
- `cdaf6c227`/`4ac0bd8c7` durable nested tool calls + structured output — kernel-side precedent for PTC evolution

## 3. Borrow from **codex-rs** (the Rust peer)

| # | What | Where | Fit / effort |
|---|---|---|---|
| 3.1 | **Patch engine**: `Hunk` parser, `seek_sequence` fuzzy matching, streaming parser, file_update over a sandbox FS abstraction | `codex-rs/apply-patch/src/` | Rust-native edit matching (replace the Python edit skill's matcher or back it). Medium |
| 3.2 | **Compaction model fallback**: summarization routes to a different model; on failure, `should_retry_with_current_model` retries with the session model (auth/error-aware) | `codex-rs/core/src/compact_model_fallback.rs` | The T05 reference: a fallback ladder instead of a hard aux-model default. Low |
| 3.3 | **Retained tool outputs**: named outputs without calls are kept whole or dropped within the history token budget | `codex-rs/core/src/compact_retained_tool_outputs.rs` | Closes the "stale tool results persist until the cut" gap (findings/19). Low |
| 3.4 | **Token-budget compaction**: a no-summary fresh-window mode modeled with the full compaction lifecycle + **pre/post compact hooks** (`Pre/PostCompactHookOutcome`) | `codex-rs/core/src/compact_token_budget.rs`, `hook_runtime` | A third engine option for the T03 debate, deterministic by construction; hooks are a clean user extension point. Low-medium |
| 3.5 | **HTTP client**: connect timeouts, `is_timeout` error mapping, `retry_after` parsing, route-aware client pool, network policy, TLS fallback, outbound proxy | `codex-rs/http-client/src/` | A02's ending (idle deadlines surfacing as `ProviderConnectionError { kind: Timeout }`) + provider robustness for pa-ai. Medium |
| 3.6 | **Context fragments pipeline**: composable fragments (base_instructions, environment_context, **current_time_reminder**, image_resize_notice, guardian_* policy fragments, inter_agent_message) merged through a `WorldState` snapshot/prefix-update model | `codex-rs/core/src/context/`, `context/world_state/` | Reference architecture for evolving the prompt layers; individual fragments (time reminder) are cheap lifts. Medium |
| 3.7 | **Compaction telemetry**: `CompactionTrigger/Strategy/Implementation/Phase` enums + backoff + analytics events | `codex-rs/analytics`, `core/src/compact.rs` | A mature version of `note_deterministic_compaction`; measures adoption of any T03 decision. Low |
| 3.8 | **Persistent memories**: read + write subsystems for cross-session memory | `codex-rs/memories/` | The real feature the harness-digest gestures at (T04's endgame). Larger |
| 3.9 | **Sandboxing**: `linux-sandbox`, `bwrap`, `mxc-sandbox`, `windows-sandbox-rs`, `execpolicy`, `process-hardening`, `exec-server` | `codex-rs/*` | The biggest capability gap this repo exposes: prime-agent runs bash unsandboxed. Strategic |
| 3.10 | Small lifts: `ansi-escape` (ANSI parsing — our tool-noise note), `rollout`/`thread-store` (session resume), `file-search` (ripgrep tool), `skills` crate, `app-server-*` (IDE protocol) | various | Opportunistic |

Notably codex counts no tokens locally (no tiktoken anywhere): budget logic runs off API-reported usage + estimates — the same posture as prime-agent, so nothing to borrow there.

## 4. Borrow from **pi-fabric** (orchestration)

- **`src/compaction/orphan-repair.ts`** — the standout: narrow tool-pair repair after cuts ("Tool result missing (repaired by Fabric)" synthetic results for unanswered calls; orphan/duplicate results dropped; error/abort-turn edge cases excluded; **returns the array unchanged when clean, so the prompt-cache prefix is never touched**). Fit: pa-core's post-compaction validity layer — cheap, high safety value.
- **`src/compaction/pressure.ts` + `threshold.ts`** — per-model threshold maps (`tokenThresholds["provider/id"]`) and a read-only pressure band (`ok|warn|urgent`) that programs act on (request compaction, narrow carry, hand off, ignore). Pairs with the fixed threshold; the band could feed the harness digest.
- **`src/compaction/{carry,qa,trace-events}.ts`** — carry-list curation, summary QA, compaction trace events — observability/testing patterns the structural engine lacks.
- **Jev fixed-cost transient-HTTP billing with retries** (8954598) — pattern for aux-call cost accounting.
- **Agents/mesh/councils/actors/durable** — strategic kernel features; the RLM API (`rlm.spawn/collect/…`) already points this direction.

## Master index — all 61 borrows

Phase 0 = substrate · Phase 1 = token/cost · Phase 2 = history/wire · Phase 3 = kernel/safety · Phase 4 = verifiers/UX · Phase 5 = strategic. Size = source LOC where measured.

| # | Item | Source | Phase | Size |
|---|---|---|---|---|
| 1 | Summarizer/gate `CacheRetention::None` + fresh routing ID | pi | 1 | few lines |
| 2 | Cache warmer + `ModelPromptCache` tiers (+ leases, 8) | pi | 1 | 453 |
| 3 | Orphan-repair after compaction | fabric | 1 | 118 |
| 4 | Compaction model-fallback ladder | codex | 2 | — |
| 5 | Retained-tool-output budgets; http-client timeout patterns (A02) | codex | 2 | 11,290 |
| 6 | apply-patch fuzzy matching | codex | 5 | 4,861 |
| 7 | Strategic: memories, sandboxing, mesh/councils | codex+fabric | 5 | — |
| 8 | repeat-guard (warn 3 / block 6) | fabric | 1 | 33 |
| 9 | Carry list (session-log entry) — lands next-steps/04 | fabric | 1 | — |
| 10 | Cache-miss attribution | pi | 1 | 174 |
| 11 | github-copilot provider | pi | 5 | — |
| 12 | Cross-process budget ledger | fabric | 3 | 215 |
| 13 | Behavioral eval harness (prompt+reload grammar) | pi | 4 | 3,329 |
| 14 | Streamable HTTP MCP transport | pi | 2 | slice of 3,244 |
| 15 | Worktree-per-subagent; foreman pattern | fabric | 3 | — |
| 16 | sPTC (needs a sandbox story) | fabric | 5 | — |
| 17 | Durable commit-before-show; memory interface; chord direction | pi | 5 | 22,842 |
| 18 | Extension-system seam (Python lane = on-ramp) | pi/fabric | 5 | — |
| 19 | DeepSWE bench with read-pathology stats | fabric | 4 | — |
| 20 | Auto-approval classifier (hard escalation) | fabric | 3 | 396 |
| 21 | beforeToolCall/afterToolCall parent-chained hooks | pi | 2 | — |
| 22 | Settings delta: cache notices, trust, tree nav | pi | 1–2 | — |
| 23 | Jev decision runtime | fabric | 5 | — |
| 24 | Effect policies for tools | fabric | 5 | — |
| 25 | Mid-conversation `tool_addition` (needs 47) | pi | 2 | — |
| 26 | Terminal images (kitty/iterm2) | pi | 4 | 763 |
| 27 | State-layer claims/evidence/certification | fabric | 3 | — |
| 28 | `parseJsonWithRepair` (partial JSON) | pi | 2 | — |
| 29 | Experimental worker patterns (session catalog, control plane) | pi | 5 | — |
| 30 | TUI polish: latex + oklab only (kill-ring/fuzzy exist) | pi | 4 | — |
| 31 | Overflow pattern library; 3.5-vs-4 calibration note | pi | 1 | — |
| 32 | Scoped `thinking.*` effort control | fabric | 3 | — |
| 33 | `pi-messages` provider (Radius wire) | pi | 5 | — |
| 34 | Failure-path output budgets | fabric | 1 | 78 |
| 35 | Strict-schema keyword filtering | pi | 0 | — |
| 36 | Native tool catalog w/ search descriptions | fabric | 2 | — |
| 37 | Bash middleware contract | fabric | 2 | — |
| 38 | Failure-progress digest | fabric | 1 | — |
| 39 | Schema-validated agent results (fixes finding 17) | fabric | 3 | — |
| 40 | Workflow helpers (agent/parallel/pipeline/budget) | fabric | 3 | — |
| 41 | Non-vision image placeholders + dedupe | pi | 0 | — |
| 42 | Mesh grants (scoped tokens) | fabric | 5 | — |
| 43 | Chunk-boundary ANSI splitting | pi | 0 | — |
| 44 | OAuth flows (openai-chatgpt, openrouter first) | pi | 5 | — |
| 45 | Classifier model support | pi | 0 | — |
| 46 | Fifteen orchestration skill patterns | fabric | 4 | — |
| 47 | Compat-field delta port (substrate for 25/32/36) | pi | 0 | — |
| 48 | Audit-trace envelope discipline | fabric | 2 | — |
| 49 | Certification harness (deterministic engine) | fabric | 4 | — |
| 50 | External CLI agent adapters (claude/codex children) | fabric | 5 | — |
| 51 | Focused-conversation views (per-child cost stats) | fabric | 4 | — |
| 52 | Chord facet/service model | pi | 5 | — |
| 53 | Tool entropy + safe normalizations | fabric | 3 | — |
| 54 | Background task inspector | fabric | 4 | — |
| 55 | Secret warnings in tool previews | fabric | 4 | — |
| 56 | WebSocket transport lane | pi | 5 | — |
| 57 | Conformance-suite pattern (storage batteries) | pi | 0/2 | — |
| 58 | ~~Per-file mutation queue~~ — **already ported** (correction #6): `pa-core/src/tools/file_mutation_queue.rs`, wired at `edit.rs:235` | pi | — | done |
| 59 | Guardian-pattern isolated reviewer (escalation tier) | codex | 3 | — |
| 60 | execpolicy command DSL (sandboxing track) | codex | 5 | — |
| 61 | Saved-program store (content-addressed, authority-free) | fabric | 3 | — |
| 62 | Actor validity predicates (bounded QuickJS liveness conditions) | fabric | 3 | — |
| 63 | Entropy v3 metric design + review-signal taxonomy (the anti-gamable core of 53) | fabric | 3 | — |
| 64 | Law shape: kernel↔spec correspondence pairs (safe+complete) as property tests | fabric | 4 | — |
| 65 | Cache-discontinuity attribution — name what busted the prefix (model/tools/reasoning/key changed) | codex | 1 | — |
| 66 | Content-based token estimates (charge item content, not transport metadata) | codex | 1 | — |
| 67 | Per-model auto-compact token limit scopes (`model_auto_compact_token_limit` + scope) | codex | 1 | — |
| 68 | MCP prewarm coalescing (invalidate-vs-reproject, bounded channel) | codex | 2 | — |
| 69 | Rollout/session-log compression with error metrics | codex | 5 | — |
| 70 | YAML skill manifests: bounded fields + UI metadata (icons, brand color) | codex | 4 | — |
| 71 | Hooks engine — declarations, registry, config rules, output spill, MCP + compact Pre/Post hooks | codex | 2 | — |
| 72 | `sandboxing` manager crate — landlock + PID namespaces + policy transforms over bwrap/seccomp | codex | 5 | — |
| 73 | `worktree` crate — Rust worktree management; grounds item 15 (subagent isolation) | codex | 3 | — |
| 74 | External-agent config migration — import Claude/Cursor hooks, MCP, memory on onboarding | codex | 4 | — |

Corrections on record: #1 queue modes already ported (pass 3) · #2 ANSI stripped at context build (pass 6) · #3 jev listing artifact (pass 8) · #4 kill-ring exists (pass 10) · #5 fuzzy exists (pass 10) · #6 file-mutation-queue already ported (pass 16) · #7 codex retained-outputs misread — it retains client-authored named outputs without calls, not stale results (shortlist validation) · #8 fabric child-tool-allowlist superseded by `write-guard.ts` confinement (shortlist validation).

## 5. Ranked for prime-agent right now (value ÷ effort)

1. **Summarizer/gate `CacheRetention::None` + fresh routing ID** — new defect (§0), few lines, direct TS parity (pa-core)
2. **Cache warmer + `ModelPromptCache` catalog tiers** — the real T02 fix, TS-proven economics (pa-models, pa-ai, pa-core)
3. **Orphan-repair after compaction** — fabric's narrow, cache-preserving shape (pa-core)
4. **`compact_model_fallback` ladder** — settles T05 without a hard default (pa-core)
5. **Retained-tool-output budgeting + http-client timeout patterns** — history hygiene (findings/19) + A02 (pa-core, pa-ai)
6. **apply-patch fuzzy matching** — Rust-native edit reliability (pa-core/tools)
7. Strategic: memories (codex 3.8), sandboxing (codex 3.9), mesh/councils (fabric)

Items 1–5 also unblock or directly answer Tier-2 items T02, T04, T05 and A02 from `validated_to_fix/README.md`.
