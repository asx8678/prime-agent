# Pass 5: the loop, the native protocol, and budget discipline

Fifth survey pass, 2026-10-11 — the last unexplored layers: the TS agent loop's stop semantics, pi's own wire protocol as a provider API, strict tool-schema munging, the durable task API, and fabric's thinking control, failure budgets, bash middleware, and tool catalog. Extends passes [02](02-pi-and-fabric-deep-dive.md)–[04](04-tui-wire-and-state.md). Nothing here is implemented.

---

## I. pi (TS upstream)

### I.1 `pi-messages` — pi's own wire protocol as a provider API

`pi/packages/ai/src/api/pi-messages.ts`: a single `POST {model, context, options}` to `<baseUrl>/messages`, answered by an SSE stream of serialized assistant-message events plus a terminal `done`/`error` — "the wire protocol spoken by the Radius gateway, but any backend implementing it can be used" (custom providers via `"api": "pi-messages"` in models.json). pa-ai has no such provider (verified against the provider list). Borrow: a `pi-messages` API provider in pa-ai for Radius-gateway parity and future gateway deployments of the Rust product.

### I.2 Strict-schema keyword filtering

`pi/packages/ai/src/api/constrained-sampling.ts`: an `UNSUPPORTED_STRICT_SCHEMA_KEYS` list (`$ref`, `$defs`, `allOf`, `oneOf`, `patternProperties`, `not`, `if`/`then`, `prefixItems`, …) plus a per-provider `UnsupportedStrictSchemaKeywordCheck` — schemas are munged/flattened before strict-mode providers reject them. Rust pa-ai sends schemas as authored. Borrow: the strict-schema filter so pa-ai tools work against strict-mode providers (Anthropic strict tools, OpenAI structured outputs) without hand-writing flattened schemas per tool.

### I.3 Agent-loop stop semantics

`pi/packages/agent/src/agent-loop.ts`: the outer loop continues on queued follow-ups after the agent would stop; a **`length` stop (output cut by the token limit) is special-cased** at :264-268 rather than treated as an error; abort signals are checked at every seam with `createErrorToolResult("Operation aborted")` for in-flight tools. Rust pa-agent ports steering and the stop-hook; the parity check worth running is the `length`-stop handling (what does the Rust loop do when the model stops on max_tokens — continue, surface, or error?).

### I.4 The durable task API

`pi/packages/durable/src/tasks.ts`: `defineTask<I, S extends { phase: string }, R, H>` — typed task definitions with **phased state** registered into a harness registry. This is the concrete shape of the task-graph borrow (pass 2 §A.2): tasks carry input/output types and a phase-tagged state machine, which is what makes mid-turn crash recovery decidable.

(The telemetry package is minimal — schemas in `index.ts`, memory/noop/testing impls — consistent with pa-telemetry's ported shape; `env`/`client` are small connection utilities already mirrored by the Rust daemon-wire client.)

---

## J. pi-fabric

### J.1 `thinking.*` — scoped effort control

`docs/thinking.md`: programs change the host session's reasoning effort for a **bounded scope** — `thinking.set({level: "high", reason: "proof step"})` for one run, `{level: "low", scope: "turns", turns: 3}`, or `{scope: "session"}`; `thinking.status()` reports level, bounds, baseline and any override; `thinking.reset()` restores the baseline. Every change goes through Pi's own `setThinkingLevel`, so it persists and clamps to the model. This is the adaptive-thinking primitive the original audit's Phase-3 item 7 asked for ("thinking/effort budgets sensible per task type"): raise effort for a hard step, drop back automatically. Rust has the `set_thinking_level` daemon switch — the borrow is the **scoped-override semantics (turns-scoped auto-reset) plus kernel exposure**.

### J.2 Failure-path output budgets

`src/output-budget.ts`: successful model/tool output keeps the configured budget, but **failures cap at 20,000 chars** (`MAX_FAILURE_MODEL_OUTPUT_CHARS`), with overflow written to a scratch **artifact** carrying `originalChars`/`omittedChars` so nothing is silently lost. The token audit checked truncation but never considered **differential budgets** — failure noise (stack traces, test logs) is exactly the low-information-per-token content finding 03's compaction summary complained about. Borrow: success-vs-failure output budgets in pa-core's tool-result truncation.

### J.3 Cooperative bash middleware

`docs/shell-middleware.md`: extension-owned `bash` overrides (an SSH backend, a security gate) keep their execution but can **attach Fabric's background-jobs capability** via a v1 middleware contract: `{spawnHook, shellPath, commandPrefix, wrapOperations: (local) => redactOutput(local)}`. Borrow: the middleware contract shape for prime-agent's bash — output filters (redaction) and alternative backends without losing backgrounding. Pairs with the extension seam (pass 3 §C.1).

### J.4 Background job semantics

`docs/background-tasks.md`: session-owned job IDs; detach on `background: true`, the shell-hang threshold, or Ctrl+B twice; terminal states `exited|failed|killed|timed_out` (no "spilled"); and the discipline line worth borrowing verbatim: **"`exited` means exit code zero, not that the user's assignment is verified."** Completion sends bounded, batched events to the owning session, including inside fabric-enabled children. Rust's kernel BashHandle covers the mechanics; run the parity check on detach conditions and terminal-state names.

### J.5 The native tool catalog

`src/native-tool-catalog.ts`: `NativeToolEntry {name, ref, description, searchDescription, declaration, inputSchema, namespace}` — every tool carries a **search description** and a namespace, the substrate for tool search/discovery. This is the catalog shape deferred tools need (pass 4 §F.2's `tool_addition`): a compact registry up front, tools materialized by value on demand, searchable by description. Borrow together with F.2.

### J.6 `failure-progress.ts` — a bounded failure digest

Formats "what failed so far" from the audit trace for the model on retries: at most 8 completed calls, paths compacted to 100 chars. Pairs with the repeat-guard (pass 2 §B.1): the guard blocks the loop, this explains it. Borrow: a failure-progress digest injected on repeated failures in pa-core.

---

## K. Ranked additions (extend the list to 38)

32. **`thinking.*` scoped effort control** — turns-scoped overrides with auto-reset; the audit's per-task effort answer (pa-core/pa-daemon)
33. **`pi-messages` provider** — Radius-gateway wire parity (pa-ai)
34. **Failure-path output budgets** — 20k cap + artifacts on failure output (pa-core)
35. **Strict-schema keyword filtering** — strict-mode provider compatibility (pa-ai)
36. **Native tool catalog with search descriptions** — the deferred-tools substrate, pairs with item 25 (pa-core)
37. **Bash middleware contract** — redaction filters and alternative backends (pa-core/pa-daemon)
38. **Failure-progress digest + `length`-stop parity check** — retry nudges (pa-core/pa-agent)

Items 32 and 34 answer audit gaps directly: 32 is Phase-3 item 7's mechanism, and 34 extends the truncation policy the audit validated in `findings/tokens-and-context/12`.
