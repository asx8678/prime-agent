# Deep dive: what to borrow from **pi** and **pi-fabric**

Second pass, 2026-10-11 — module-level survey of the TS upstream and the Fabric runtime, beyond [README.md](README.md). Cross-referenced against `findings/tokens-and-context/`, `validated_to_fix/`, and prime-agent's own `next-steps/` roadmap. Nothing here is implemented.

---

## A. pi (TS upstream, `cdaf6c227`)

### A.1 Provider fleet gap — TS ships ~35 providers, Rust has 11

`pi/packages/ai/src/providers/` vs `crates/pa-ai/src/providers/` (anthropic, azure-responses, bedrock, faux, google, google-vertex, mistral, openai-codex-responses, openai-completions, openai-responses, systemone). Highest-value gaps:

- **`github-copilot.ts`** — OAuth (subscription) + `COPILOT_GITHUB_TOKEN` env auth, dynamic Copilot headers (`github-copilot-headers.ts`), and `filterModels` by credential type. A real provider with a real auth flow, not just openai-compat plumbing.
- **`openrouter.ts`, `vercel-ai-gateway.ts`, `cloudflare-ai-gateway.ts`** — gateway providers users actually have keys for.
- The long tail (deepseek, groq, cerebras, together, xai, fireworks, minimax, moonshot, qwen-token-plan, xiaomi-token-plan, kimi-coding, zai, …) is mostly openai-completions-shaped: the borrow is **catalog metadata + auth + per-provider quirks**, not new protocol code.
- `radius.ts` — the Radius gateway provider (`PI_RADIUS_GATEWAY`) with an MCP sign-in flow.

### A.2 The `durable` package — where upstream is investing

`pi/packages/durable/` — "a durable agent harness: conversations, model turns, tool calls, and your own state are committed to storage **before anything is shown**. If the process dies mid-turn, reopening the storage picks the work up where it stopped." Structure: `harness/` (agent, compaction, context, events, generation, inbox, live, output, prompt), `session/`, `storage/`, `tasks.ts`, `documents.ts`, `images/`, `tools/`, `testing/`.

Recent commits are almost all here: nested tool calls with one document per call, scheduler ownership indexes bounded by live work, `documentRetentionMs` cache bounding, image prep in a worker thread.

Borrow for prime-agent: the **commit-before-show** model for daemon workers (prime-agent's JSONL persists at turn boundaries; a mid-turn crash loses the in-flight turn), and the **task-graph / owned-tasks** concepts for RLM children. This is an architectural lift, not a drop-in — but it is the upstream direction.

### A.3 The `evals` package — the verifier prime-agent lacks

`pi/packages/evals/` — behavioral evals on `vitest-evals` with a harness (`src/harness.ts` creates full coding-agent sessions in tmpdirs), plus suites: `smoke.eval.ts`, `documentation-audit.eval.ts` (a structured-verdict tool with `constrainedSampling` + `terminate: true` — the model must submit `{verdict: match|mismatch|inconclusive, explanation}` as typed data), docs audits for providers/TUI/extensions, `configured-runtime.ts`, `acme-server.ts`.

Borrow: a faux-provider behavioral eval harness for prime-agent. It directly closes findings/14's gap (the LLM compaction engine's summary quality has **never been tested** — only canned mock text) and serves the repo's "verifiers over self-assessment" rule. The documentation-audit pattern (typed submission tool as the eval's verdict channel) is the design to copy.

### A.4 `codemode` — the other PTC

`pi/packages/codemode/` — model-written JavaScript in a QuickJS/WASM VM where the only capability is injected tools; **nested tool calls never enter the LLM context, only the script's output does**. Scripts get `tools`, `ALL_TOOLS`, `text`, `image`, `exit`, **`store`, `load`**, and a `// @options:` line. Zero pi dependencies.

Prime-agent's surface contract pins the ipython kernel, so this is not a drop-in — but **`store`/`load`** (persist kernel state across calls) and the options-line are cheap kernel-lane borrows, and codemode is the reference for a future lightweight lane.

### A.5 `pi-mcp` transports

Transport-neutral MCP client core with **stdio, Streamable HTTP, and an in-memory testing transport**, no MCP SDK dependency. Rust has `crates/pa-core/src/mcp/` (catalog/probe/views) with no Streamable HTTP transport — borrow it.

### A.6 `protocol`, `server`, `chord` — strategic wire/composition

- `protocol` — runtime-neutral routed envelopes, **CBOR encoding**, byte-stream framing, protocol version 8 (the experimental future wire; pa-daemon's current wire is parity-pinned to the TS daemon — treat as reference).
- `server` — local server routing clients to application-hosted durable Sessions (facet-service routing, presentation-scoped capabilities).
- `chord` — application-composition runtime: facets, services, replicated state, pluggable remote-service boundary. The extension substrate under coding-agent.

### A.7 TS core modules with no Rust counterpart

`pi/packages/coding-agent/src/core/` items prime-agent lacks:

| Module | What it does | Borrow value |
|---|---|---|
| `cache-stats.ts` | Per-turn cache-**miss** attribution: idle gaps > 5 min flagged as TTL misses vs the 1,024-token breakpoint-granularity noise floor | Our audit could not measure hit rate at all; this is the visibility layer for findings 01/02. pa-telemetry/pa-core |
| `nested-tool-calls.ts` | Tools calling tools via `ctx.executeTool()` with `parentToolCallId`, own hooks, usage recorded on the parent call's result | Attribution model for kernel-side nested calls |
| `project-trust.ts` + `trust-manager.ts` | Trust store + gating for project resources (extensions, MCP, packages) with trust events | Security-relevant: prime-agent honors `<cwd>/.prime/agent/settings.json` (the path-traversal attack vector we just fixed) with no trust gate |
| `cache-warmer.ts` + the SDK capability `ctx.acquireCacheWarming("idle")` | The warmer (first report §2.1) plus its extension-facing lease API — Fabric builds `cache.hold/release` leases on it | Borrow both halves together |
| `output-guard.ts` | Stdout takeover hygiene for extension output | TUI terminal safety |
| `bug-report.ts`/`bug-report-upload.ts`/`crash-log.ts` | One-command bug reports with uploads | Support surface |
| `keybindings-schema.ts`/`key-id-schema.ts` | Machine-readable keybinding/key-id schemas | Config validation |
| `remote-catalog-provider.ts`, `models-store.ts` | Remote model catalog | Pairs with `ce950d78f` (models.dev metadata) for pa-models |
| `sdk.ts` | The composition surface for embedding | Reference for a Rust SDK story |

### A.8 Recent upstream commits worth porting

`f44707e7f` documentRetentionMs bounding · `6b07b4e57` Mistral header-only timeout (the A02 reference) · `ce950d78f` models.dev catalog metadata · `f1b2e77f5` symlinked AGENTS.md in nested worktrees · `4ac0bd8c7` durable nested tool calls + structured output.

---

## B. pi-fabric (`9cea25d`, 0.112.0)

### B.1 `repeat-guard.ts` — finding 09, answered in production

Warn at **3**, block at **6** consecutive identical `fabric_exec` submissions; fingerprint deliberately **excludes display names and payload cosmetics** ("cosmetic variation is part of the loop signature"); any differing code resets the count. Motivated by a real incident (2026-09-15: 220× `bash echo noop` on glm-5.3-flash). Port the thresholds and fingerprint approach to pa-core's kernel path — this is exactly the loop-detection `findings/tokens-and-context/09` said was missing, with production-tuned constants.

### B.2 The compaction suite — prime-agent's `next-steps/` roadmap, already built

- **`compaction/carry.ts`** — a bounded carry-forward fact list stored as a **session-log custom entry** (replay-safe, no in-memory copy to lose), rendered into **every** deterministic summary until cleared; updates support add/remove/clear with `MAX_PRESERVE_ITEMS`/`MAX_PRESERVE_ITEM_CHARS` bounds. This is `next-steps/04-pr5-carry-and-preserve.md`, designed and shipped.
- **`docs/compaction.md`** — `compaction.targetContextRatio` (default 65%, bounded 0.25–0.85): a hard **post-compaction occupancy ceiling** that is "never treated as space to fill" — a cleaner trigger-side policy than a keep-window alone.
- **`docs/extractive-history.md`** — classifier-assisted **verbatim source evidence** from the active branch: off by default, explicit consent + cost warnings, native-classifier catalog selection, thinking/tool-output/summary-prose excluded from classifier input, no cross-session retrieval. This is the design shape for `next-steps/02` (the `[u12]`/`[u12.c3]` address expansion) and `05-context-folding`.
- **`orphan-repair.ts`**, **`pressure.ts`** bands, per-model `threshold.ts` maps, **`qa.ts`**, **`trace-events.ts`** — already in [README.md](README.md) §4.

### B.3 `agents/budget-ledger.ts` — subagent cost budgets

A cross-process **append-only JSONL cost ledger for a recursion tree**: each node records child costs and checks accumulated spend before spawning another child; the ledger path + budget travel via `PI_FABRIC_BUDGET*` env; best-effort (concurrent children can overshoot) with the race-free ceiling being per-execution call counts; append-after-completion semantics. Direct borrow for RLM children — pairs with the 64 KiB answer-cap finding (17) and `--goal-token-budget`.

### B.4 `agents/` worktree isolation

`bash-worktree-add.ts` (safe `git worktree add` parsing with a `BARE_SAFE` arg allowlist), `clone-first-worktree.ts`, `cow-clone.ts` — **per-subagent git worktree isolation**. Prime-agent's RLM children share the parent cwd; parallel editing subagents would collide. Borrow for child isolation (and note the arg-allowlist pattern after our path-traversal fix).

### B.5 `prewalk/` — fs-drift detection

`arm/controller/fs-drift/handoff/model-switch/plan/return` — a pre-flight walk that detects **filesystem drift** between plan and execution, handles model switches mid-flight, and manages handoffs. Borrow: fs-drift detection for edit reliability (files changed under a stale plan).

### B.6 `repairs/` — model-mistake repair instead of failure

An argument/action **repair compiler**: `classify`, `promote`, `store`, `catalog-digest`, `active` — repairs malformed tool args/actions instead of erroring. Borrow the pattern for kernel-side arg repair (cuts retry round-trips; pairs with our edit-failure findings).

### B.7 `memory/` — the portable memory interface

`defineMemorySource`, a registry, and an explicit host client (`pi-fabric/memory` subpath, no UI/command runtime loaded). This is the pluggable interface the Continual Harness's `memory:` count gestures at — the T04 endgame.

### B.8 `docs/prompt-cache.md` — cache leases

Fabric exposes `cache.status / cache.hold / cache.release` (+ host `cache.lease`) above Pi's native warming capability: a hold survives the allocating execution and can be released later; "a lease cannot force a refresh or guarantee a cache hit"; never simulates warming with prompts. Borrow the lease surface together with A.7's warmer capability.

### B.9 `docs/speculation.md` — speculative PTC (sPTC)

Pre-launch tool calls **while the model is still streaming the program**, so tool latency overlaps token generation, with a precise correctness contract: "a served speculative result is indistinguishable from the call having executed at its natural program point." Kernel support: QuickJS TS, Python/Monty, **CPython only under schema-enforce** (sandbox required — native CPython is bypassed because ambient OS/network mutations break the equivalence). Latency + token win for prime-agent's PTC lane, but the CPython caveat means it needs the sandbox precondition first.

### B.10 `docs/foreman.md` — a cheap supervisor above the coding loop

A fast **typed decision loop** (Jev program) watching Main at `agent_settled`/`turn_end` decision points — "no second generative model runs for every observation." Built entirely from existing primitives (spawn/observe/events). Borrow the pattern via the RLM kernel: an economical replacement/adjunct for the auto-refine gate (findings/05's ~41 KB LLM gate per compaction becomes a typed observer).

### B.11 The `docs/` design library

40+ design docs: `decisions.md` + `decision-profiles.schema.json` (schema-enforced decision profiles), `component-calculus`, `schema-enforcement`, `state-layer`, `residency-runtime`, `audit-trace`, `certification`, `verified-kernels`, `codemode-parity`, `background-tasks`, `programs`, `provider-capabilities`. Mine it before designing any of the above.

---

## C. Ranked additions (extend README §5)

8. **repeat-guard** — finding 09's answer, production-tuned constants (pa-core kernel)
9. **Carry list** — `next-steps/04` already designed in fabric (pa-core compaction)
10. **cache-stats miss attribution** — visibility for findings 01/02 (pa-telemetry)
11. **github-copilot provider** — the one provider gap with a real auth flow (pa-ai)
12. **budget-ledger** — RLM child cost ceilings (pa-daemon)
13. **Eval harness** — closes finding 14's untested-LLM-compaction gap; serves the verifier rule (new pa-core test infra)
14. **Streamable HTTP MCP transport** — from pi-mcp (pa-core/mcp)
15. **Worktree-per-subagent + foreman pattern** — RLM child isolation and cheap supervision (pa-daemon/pa-core)
16. **sPTC** — speculative PTC, gated on a CPython sandbox story (kernel)
17. Strategic: durable commit-before-show, memory-source interface, chord/protocol/server directions

Items 8–10, 13 also close or instrument open findings (09, 14, 01/02); item 9 lands an already-planned next-step.
