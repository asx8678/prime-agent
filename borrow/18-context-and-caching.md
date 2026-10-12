# Pass 18: how the sources manage context and caching

Eighteenth pass, 2026-10-11 — the consolidated context-management chapter: request assembly, prompt-cache placement and TTL policy, warming, model-catalog caching, history hygiene, and cache-bust attribution, across pi / fabric / codex, with prime-agent's deltas. Extends [02](02-pi-and-fabric-deep-dive.md)–[17](17-bend-entropy-mesh-actors.md). Nothing here is implemented.

---

## AS. Request assembly — the living system message (pi)

The TS system prompt is not a static string: the **initial system message** (text + named `sections` + `toolsAdded`) is transcript message #1, later **system messages carry updates**, and `getCurrentSystemMessage`/`getCurrentTools` fold them into the current state (`utils/transcript.ts`). Two resolution modes: APIs with mid-conversation system support keep later messages in place; `collapseSystemMessages` **replays the folded current system message first and drops the rest** for APIs without it (`resolveTranscript`, compat-gated). Tools travel *inside* the system message — which is how mid-convo tool additions keep the request-level tool list fixed and the cached prefix intact (pass 4 §F.2). History resolves through the session tree with compaction boundaries and branch summaries; `transform-messages` downgrades images by model capability (`model.input.includes("image")`) with consecutive-placeholder dedupe.

## AT. Prompt caching (pi)

- **Placement** (anthropic): `cache_control` on the system block(s), the last tool, and the last user message — 3 breakpoints; OAuth adds a Claude-Code identity block.
- **Retention**: `short` (5 m) default; `long` (1 h) via option or `PI_CACHE_RETENTION`; `none` for one-offs. The `ttl: "1h"` field is sent **only** when long *and* `supportsLongCacheRetention`.
- **Per-model cache metadata**: `model.promptCache[retention]` seconds (`ModelPromptCache`) — the warmer's clock and the catalog's contribution.
- **The warmer**: refresh at **90 % of TTL minus 10 s margin**; only when expected saving ≥ **$0.05**; idle-continuation probability **0.15**; replay with a **one-token output cap**, guarded by `isReplayable` (Anthropic derives `budget_tokens` from `max_tokens` on non-adaptive models — a naive replay would key a different cache entry and still think).
- **Miss attribution** (`cache-stats.ts`): per-turn misses labeled **TTL-gap** (> 5 min idle) vs **breakpoint-granularity noise** (≤ 1,024 tokens); surfaced to users via `showCacheMissNotices`.
- **One-offs opt out**: summarization sets `cacheRetention: "none"` + a fresh routing id — the exact fix of backlog PR 1.
- **Per-provider keys**: openai-completions sends `prompt_cache_key` (api.openai.com or Long+compat) with `prompt_cache_retention: "24h"` on the compat path; responses/codex APIs send `prompt_cache_key = session_id`; Bedrock mirrors the retention resolution.

## AU. Model caching (pi)

- **models.json store** (`models-store.ts`): file-**revision-keyed single-flight reloads** — one in-flight reload per path with an abort controller and reader count, shared read state, BOM stripping. No staleness windows; the file's revision is the cache key.
- **Remote catalog** (`remote-catalog-provider.ts`): `https://pi.dev`, 4 s attempt timeout, **4-hour refresh**, `?types=` full-type-shard negotiation with graceful degradation to the chat-only shard.
- **Faux provider**: an in-memory session-keyed prompt cache for tests — previous prompt's common prefix becomes `cache_read`, the remainder `cache_write`, exercising the accounting without a network.

## AV. Fabric's layer

- **`cache.*` capability** (`src/cache/{leases,observations}.ts`): status/hold/release above pi's native warming — a hold survives the allocating execution; never simulates warming with prompts; a lease cannot force a refresh.
- **Context hygiene**: the audit-trace envelope rule — session JSONL never holds logs, return values, or media; the privacy-projected trace feeds compaction and memory (pass 7 §P.4). `targetContextRatio` caps post-compaction occupancy; pressure bands expose fullness without compacting.

## AW. Codex — stateful context + bust attribution

Codex rides the Responses API's **`previous_response_id` chaining** (server-side context) and does the thing neither pi nor fabric does: a **discontinuity classifier** (`client.rs:385-448`) compares the previous request to the current **field by field** — model, tools, tool_choice, parallel_tool_calls, reasoning, store, stream, include, service_tier, `prompt_cache_key`, text — and returns a **named reason**: `model_changed`, `tools_changed`, `reasoning_changed`, `prompt_cache_key_changed`, … That is cache-bust *attribution* at the wire level: every context break carries its cause. (Plus `cached_tokens` accounting in sessions and per-reviewer cache keys for guardian sessions.) Codex counts no tokens locally — budgets run off API-reported usage.

## AX. Prime-agent's deltas (from the audit + passes 10/16)

Already at parity: single cached system block (the TS non-mid-convo shape), 3 breakpoints, short-TTL default, `PI_CACHE_RETENTION`, OpenAI-compat key gating, Bedrock retention, usage accounting incl. cache multipliers. Gaps, in dependency order: **47** compat flags (mid-convo, tool-search) → **25/36** tool additions + catalog → **2** warmer + `ModelPromptCache` → **10/22** miss attribution + notices → **48** envelope hygiene → **65** (below). Plus the sharpened PR 1 bug: the summarizer's options default *resolves* retention instead of opting out (pass 16).

---

## AY. Ranked addition

65. **Cache-discontinuity attribution** — the codex classifier shape: on every request, diff the previous request's cache-relevant fields and name the break (model/tools/reasoning/key changed). Cheap, pure derivation, and it makes items 10/22's misses self-explaining — feeds telemetry as `cache_bust_reason` (pa-core/pa-telemetry, Phase 1)

Sixty-five items. The context chapter closes the loop the audit opened: measurement (findings/12) → upstream mechanics (this pass) → PR backlog (15/16) → attribution (65).
