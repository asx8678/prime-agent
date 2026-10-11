# 5. Context folding (F1-F3) and the evaluation

Total about a week. Highest risk. Ship behind a setting, and make it a
default only if the evaluation shows a gain: the Context-Folding paper's
large gain came from RL-training the model to fold, and these models are
not trained for it. AgentFold was considered and skipped.

## F1: `fold.begin` / `fold.end` (4-6 days, high risk)

**API.** A gated bundled skill `skills/fold`:
- `await fold.begin(description)` opens a fold;
- `await fold.end(message)` closes it with a short result.

The host refuses nested folds and an `end` without a `begin`.

**Markers.** The begin and end cells carry typed markers in their saved
tool-result `details`. Details are copied into the loop's messages
(`create_tool_result_message`, `crates/pa-agent/src/agent_loop/tool_call.rs:312`),
so the markers survive into every request and through resumes.

**Applying the fold per request.** `transform_context` runs before every
model request (`crates/pa-agent/src/agent_loop/response.rs:88`). The engine
wires a pass-through today
(`crates/pa-core/src/session_engine/engine.rs:646`:
`instrument_transform_context(..., pass_through_transform())`). Replace the
pass-through with the fold view, keeping the timing instrumentation:
- drop the work between a closed fold's begin and end cells, leaving the
  end cell's message;
- the session log is never changed;
- everything before the begin cell stays byte-identical, so the prompt
  cache still hits.

**Token count.** Apply the same view in `auto_compaction_due`
(`crates/pa-core/src/session_engine/compaction_arms.rs:26`), so a fold
also delays compaction.

**Auto-close.** Close an open fold on compaction and when it grows past a
size limit. Store a hidden custom message holding a deterministic summary
of the range. Reuse the structural compactor's renderer with a small byte
budget (`structural_compaction`, `Fold::summarize`).

**With compaction.** Closed folds become verbatim blocks in the summary.
The facts inside them still count in the state, and compaction never cuts
inside an open fold.

**Opt-in.** Start with a `contextFolding.enabled` setting: a typed field,
like `compaction.engine` in `crates/pa-core/src/settings/types.rs`, so a
settings save keeps it. Add a slash command only if folding graduates.

**Tests.** The request prefix up to the begin cell is byte-identical before
and after `fold.end`. Also cover refused nesting, auto-close on compaction
and on size, and resume with an open fold.

**Telemetry.** Folds per run, tokens folded, auto-closes.

## F1b: auto-close on new input (1-2 days, medium risk)

Close an open fold when a new user prompt, steering message or external
input arrives. Add this at prompt admission in `pa-daemon` and in `pa-cli`'s
`print_runtime`.

## F2: an Anthropic cache marker at the fold start (1-2 days, low risk)

Place a cache breakpoint at the begin cell, so the prefix up to it stays
cached across the fold. In `pa-ai`, `anthropic/convert.rs`.

## F3: `fold.consolidate` (1-2 days, low risk)

The model marks closed folds to merge; they are applied at the next
compaction (`skills/fold` plus the structural compactor).

## Evaluation (3-5 days)

Run an A/B comparison on long-horizon tasks: engine (`llm` versus
`deterministic`) and folding on or off. Measure success rate, tokens and
compactions per task. Use it to decide whether folding becomes a default,
and whether the deterministic engine stays the default.

## Gates

- New REPL calls change the surface the model sees (`AGENTS.md`, "Surface
  contract"). Document the API only in the gated skill and leave the static
  system prompt unchanged.
- Record the divergence from TS in each commit message.
