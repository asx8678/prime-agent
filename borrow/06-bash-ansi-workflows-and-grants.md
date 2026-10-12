# Pass 6 (final): bash hygiene, workflows, grants — and a correction

Sixth and final survey pass, 2026-10-11. Closes the survey: bash/ANSI pipeline parity, the non-vision image transform, the OAuth flow inventory, fabric's workflow layer with schema-validated agent results, mesh grants, and the prewalk/mesh mechanics. Includes one correction to the earlier record. Nothing here is implemented.

---

## L. pi (TS upstream)

### L.1 ANSI sanitization — correction to the pass-1 record

The pass-1 note ("no stripping in the bash pipeline; `strip_ansi` applies at TUI render only") was **wrong**: Rust strips ANSI at context build — `pa-core/src/tools/render_utils.rs:228-254` runs every tool-result text block through `strip_ansi` + `sanitize_binary_output` before it joins the context. TS strips at **capture** instead (`bash-executor.ts`: `stripAnsi`, `sanitizeBinaryOutput`, `truncateTail`, `fullOutputPath` on overflow). Both pipelines end clean in context; the residual differences worth borrowing are **`splitIncompleteAnsiSuffix`** (chunk-boundary-safe stripping during streaming — a partial escape sequence split across chunks) and capture-time stripping (the session JSONL stores clean output, not escape-riddled bytes).

### L.2 Non-vision image placeholders

`pi/packages/ai/src/api/transform-messages.ts`: when the model does not support images, image blocks become `"(image omitted: model does not support images)"` (a distinct form for tool images), and **consecutive placeholders are deduped** so a vision-heavy history routed to a non-vision model doesn't fill the context with repeated placeholder lines. Prime-agent's image_route swaps *to* a vision model when attachments appear, but the fallback direction (non-vision session carrying image history) deserves the same placeholder + dedupe treatment. Parity check + small borrow.

### L.3 The OAuth flow inventory

`pi/packages/ai/src/auth/oauth/`: `anthropic`, `github-copilot`, `kimi-coding`, `meta`, **`openai-chatgpt`**, `openai-codex`, `openrouter`, plus a shared `device-code` flow and `callback-server`. Rust's auth covers the providers it ships; every provider-fleet borrow (deep-dive §A.1) brings its flow along — `openai-chatgpt` and `openrouter` OAuth are the highest-impact missing ones.

### L.4 The Agent class hook surface

`pi/packages/agent/src/agent.ts` composes `runAgentLoop`/`runAgentLoopContinue` with the full hook set: `BeforeToolCall`/`AfterToolCall` (parent-chained, pass 3 §C.3) plus **`PrepareRequest`, `PrepareNextTurn`, and `FinishTurn`**. Rust pa-agent ports steering and the stop-hook; the prepare/finish hooks are the remaining seam — and `PrepareRequest` is the natural home for a Rust cache-warmer's last-chance refresh.

---

## M. pi-fabric

### M.1 Workflows with schema-validated agent results

`docs/agents.md`: `workflow.agent/parallel/pipeline` (with concurrency), `workflow.phase/item/event/log` for progress surfaces, `workflow.budget` for token observations, per-execution `agentBudget`/`tokenBudget` limits — and the standout: **"Add a JSON Schema to an agent request to make the worker return validated structured data in `result.value`."** Without a schema the agent returns final text.

This is the principled fix for finding 17's 64 KiB answer cap: bound subagent results **by schema**, not just by bytes — `rlm.spawn` with a schema returns typed, validated values, and the byte cap becomes a backstop instead of the only limit. Python uses the same surface (`agents.run`, native asyncio.gather programs).

### M.2 Mesh grants — the external-access model

`src/mesh/grants.ts`: scoped bearer tokens that let an outside process append to **exactly one mesh topic**; only the token's SHA-256 is stored in a private `grants.json`; every mutation runs under the mesh lock with atomic use-count decrement; TTLs bounded 60 s–30 days; 64 KB external-data cap. A clean, minimal capability model — the reference for any future multi-process/mesh surface in prime-agent.

### M.3 Prewalk arm mechanics

`src/prewalk/arm.ts`: one arm path shared by the `/fabric prewalk` command and `alwaysRearm` session auto-arm, keeping the drift baseline, the hidden armed advisory, and the status chip from diverging between entry points. The design note to keep: **one arm path, however many entry points**.

### M.4 The failure taxonomy discipline

`docs/architecture.md`: "Effect failures (bash, edit miss) and guest typecheck are **classified for status but never promoted**" — failures are labeled for the status surface but never silently upgraded to success or treated as model-visible progress. Worth adopting as a stated invariant wherever prime-agent classifies outcomes (it is the same discipline as our fixed `NoExitCode` label: truthful, never embellished).

### M.5 The architecture map — the reference for the kernel's future

`docs/architecture.md` gives the one-page map of the whole runtime: `fabric_exec` → TypeScript checker → **QuickJS sandbox by default** (fresh context per execution; no `process`/`require`/fs/network/subprocess; every effect crosses a JSON-only host bridge with schemas, approvals, audit records, timeouts, cancellation) with a disposable Node process as the unsafe opt-in; an `ActionRegistry` fed by a `ComponentSupervisor` through staged commits; the namespace surface `pi.* / extensions.* / mcp.* / agents.* / mesh.* / schema.* / memory.* / compact.* / cache.* / thinking.* / decisions.* / programs.* / components.*`; a catalog repair table for unknown actions; session JSONL as the log plus `current.json` for unique schema maps. This is the target architecture prime-agent's RLM kernel is converging toward — worth keeping pinned beside the extension-seam decision (pass 3 §C.1).

---

## N. Ranked additions (final: 39–44)

39. **Schema-validated subagent results** — bound answers by schema, bytes as backstop; pairs with finding 17 (pa-daemon/kernel)
40. **Workflow helpers** — agent/parallel/pipeline/phase/budget with per-execution budgets (kernel)
41. **Non-vision image placeholders + dedupe** — parity check and small borrow (pa-ai)
42. **Mesh grant model** — SHA-256-stored scoped tokens, single-topic append (future mesh surface)
43. **`splitIncompleteAnsiSuffix`** — chunk-boundary-safe ANSI stripping for streaming (pa-core)
44. **OAuth flows with the provider fleet** — `openai-chatgpt`, `openrouter` first (pa-ai)

Correction recorded: L.1 supersedes the pass-1 claim that Rust strips ANSI only at TUI render — `render_utils.rs` strips at context build; the audit's underlying question is answered and closed.

---

**Survey complete.** Six passes, 44 ranked borrows, both repos exhausted at the module level. The borrow folder now documents: what prime-agent already has (and where the earlier record was wrong), what it lacks, and which upstream designs answer its open findings.
