# 5. Context folding (F1-F3) and the evaluation

Revised 2026-10-12 after reading the code paths it touches (see "What
changed from the first draft" at the end). Ship behind a setting, and make
it a default only if the evaluation shows a gain: the Context-Folding
paper's large gain came from RL-training the model to fold, and these
models are not trained for it. AgentFold was considered and skipped.

The step-by-step build plan, with the commit order, files and verifiers,
is `05b-context-folding-build-plan.md`; this file is the design it follows.

Naming: `structural_compaction::Fold` already exists (it folds session
events into the compaction state). Use `context_fold` / `ContextFold*` for
everything here so the two never get confused.

## What the paper and its code actually do

Source: Sun et al., "Scaling Long-Horizon LLM Agent via Context-Folding",
arXiv 2510.11967 (v1, Oct 2025; ICML 2026), and the authors' reference code
`github.com/sunnweiwei/FoldAgent` (`agents/fold_agent.py`, `prompts.py`;
its README notes it is a re-implementation that may differ from the paper's
training code).

**Mechanics.**
- Two tools. `branch(description, prompt)`: `description` is "a concise 3-5
  word identifier for the sub-task"; `prompt` is "clear, compact task prompt:
  state objectives and critical info to preserve in the response". Both are
  required. `return(message)`: "a comprehensive message describing sub task
  outcome".
- A branch is a fork: it starts from the full main history plus a role-change
  message (`BRANCH_MESSAGE`) telling the model to do only the assigned task
  and to report the outcome, files changed, environment changes, key
  insights, assumptions and open questions.
- On `return`, the main thread sees only its own `branch` call followed by
  the templated observation `Branch has finished its task, the returned
  message is:\n\n{message}`. Everything in between is gone; at inference
  "it rolls back the KV-cache to the corresponding branch position". This is
  the same prefix-identity rule as our view.
- No nesting: "creating new branches is disabled while in execution state".
  A branch that calls `branch` or `finish` is told: "You are in branch mode
  and cannot branch task or finish the task. Use the `return` tool to go back
  to the main agent."
- Every tool observation inside a branch gets this line appended: "You are
  now in branch mode: {description}. Conduct the sub task based on
  instruction, and when you complete the assigned sub task, use return tool
  to return, do not perform action beyond the assigned sub task."
- Each branch has a token budget (`branch_len`). When it runs out, the
  branch is stopped and told: "The context limit has been exceeded for the
  branch. Please finish the sub task directly and clearly state the progress
  made and the pending jobs of the sub task. Only summarize the sub task
  progress, using the return tool." A branch that ends without `return` has
  its last message used as the result.
- At most 10 branches per task in training (a refusal message after that);
  unlimited in the task-complexity experiment.
- The system prompt splits the roles: MAIN plans, orchestrates and verifies
  and "does not conduct specific work"; BRANCH executes. The coding prompt
  marks which phases to branch: exploration, test creation, fix
  implementation, code review, verification and comprehensive testing are
  `[BRANCH]`; planning and final review stay in main.
- Parallel branches were tried and did not beat single branches.

**Training (does not transfer to us).** FoldGRPO adds token-level process
rewards: -1 on main-thread turns that are not `branch` calls once the main
context passes 50% of the window ("unfolded token penalty"); -0.2 on a
branch that a judge model (GPT-5-nano) finds went outside its assigned
sub-task ("out-of-scope penalty"); -1 on failed tool-call turns. Base model
Seed-OSS-36B, 32K context per thread, up to 10 branches.

**Results that matter for an untrained model** (Table 1, Pass@1, same 36B
model, before RL):

| Agent | BrowseComp-Plus | SWE-Bench Verified |
|---|---|---|
| ReAct, 32K context | 0.286 | 0.436 |
| Summary agent (compaction), 32K x 10 | 0.386 | 0.488 |
| Folding agent, 32K x 10, prompt only | 0.420 | 0.492 |
| ReAct, 327K context | 0.478 | 0.552 |
| Folding agent after FoldGRPO | 0.620 | 0.580 |

Before RL the model folds poorly on coding: main-thread length 47,475 tokens
(vs 8,885 after FoldGRPO) and scope accuracy 0.473 (vs 0.754) on SWE-Bench
(Table 2). So prompt-only folding is about as good as compaction, and worse
than simply having a long context. Our models have long windows and already
compact, so the realistic gain without training is tokens and cost, not
accuracy. The evaluation must show success rate does not drop.

**What we take from it** (applied in F1 below): a required `goal` argument;
role-change instructions returned by `begin`; a templated result; the
per-result branch-mode reminder; a per-fold budget that asks the model to
close with progress and pending work; refusing nesting with the paper's
wording; and the MAIN/BRANCH guidance with which kinds of work to fold.
What we do differently: the fold happens inside one thread (begin and end
cells, hidden by the per-request view) instead of a separate forked agent,
because the REPL, kernel state and transcript are shared and a forked
`Agent` would need its own tool loop and persistence.

## F1: `fold.begin` / `fold.end`

### API

A gated bundled skill `skills/fold`, a thin wrapper over `rlm.host_request`
like `skills/compact`:

- `await fold.begin(description, goal)` opens a fold. `description` is a
  3-5 word label; `goal` (required) states the objective and what the
  result must preserve, as the paper's `prompt`. Both stay visible after
  the fold, so the main thread keeps a record of what was delegated.
- `await fold.end(result)` closes it; `result` is the outcome the model
  keeps in place of the folded work.

`fold.begin` prints the role-change instructions, a compact version of the
paper's `BRANCH_MESSAGE` (about 80 tokens, because the begin cell stays in
context): work only on the goal; finish with `fold.end` whose result covers
the outcome, files changed or created, environment changes, key findings,
assumptions and open questions. `fold.end` prints a fixed template, `Fold
finished: {description} ({n} messages, ~{tokens} tokens folded).
Result:\n\n{result}`, so later requests see deterministic text.

Both *print* and return `None`. The model sees a cell's stdout, stderr and
the `repr` of its last expression (`repl.py:676`); display `text/plain`
payloads never reach it. A returned string would show as an escaped `repr`
with quotes and `\n`, and would be lost entirely when the call is not the
cell's last expression. Printed text lands in `details.stdout`, which the
ipython card already renders as ordinary cell output.

The host refuses nested folds and an `end` without an open `begin`, with a
reason the model can act on (see "Host state" below). The nesting refusal
uses the paper's wording: a fold is open, finish it with `fold.end` before
starting another.

### Branch-mode reminder and the per-fold budget (added by the view)

While a fold is open, every tool result after the begin cell (ipython, bash,
edit) gets one line appended, adapted from the paper: `[fold open:
{description}] Work only on this goal; when it is done, call
fold.end(result).` Once the span passes the budget, the line changes to the
paper's over-budget prompt: stop, and call `fold.end` with the progress made
and the pending work.

**Add it in `fold_view`, not in the stored result.** The loop's
`after_tool_call` hook (`agent_loop/tool_call.rs:250`; the engine leaves it
unset) could write the line into the result, but the merged result is what
`emit_tool_execution_end` sends to the UI and what the session log keeps. The
bash card renders `content`, and the ipython card falls back to `content`
for a cell with no stdout/result (`pa-tui/src/tool_card/ipython/output.rs:60`),
so the line would appear in the TUI: a visible change needing parity
evidence. In the view it reaches only the model.

It stays cache-stable as a pure function: a tool result at position *i* gets
the line when a begin precedes it with no end or voiding row in between,
and the over-budget variant when the span's chars/4 estimate up to *i*
passes the budget. Both depend only on messages up to *i*, so a result's
bytes never change between requests. When the fold closes, those results
are dropped anyway. A fold that is later voided keeps its reminders, since
membership was fixed at *i*. The begin and end cells themselves get no line.

Budget: a `contextFolding.maxFoldTokens` setting (chars/4 estimate of the
open span), default 32,768 to match the paper's per-branch context, tuned by
the evaluation. The model, not the host, writes the summary; if it ignores
the nudge, compaction is the backstop.

A turn that ends with a fold open is not forced closed. The paper refuses
`finish` inside a branch; here the reminder line already says to close the
fold first, and a fold left open is voided by the next input (rule 3
below). If the evaluation shows many voided folds, add the paper's refusal
as one continuation through `get_continuation_messages` in goal mode only.

### Marker channel: per-cell display MIME (the `diffs` path)

`HostRequestPayload` carries no cell or tool-call id (only
`cellSourceCode`, `crates/pa-core/src/kernel/manager/host_requests.rs:89`),
so a host handler cannot stamp its own cell's tool result. Use the path PR 0
(`016489755`) built for `edit()` diffs instead:

1. The skill calls `host_request("fold.begin" | "fold.end", ...)`; the host
   validates and answers.
2. On success the skill calls `repl.emit({FOLD_MIME: {...}})`, tagged with
   the running cell. New constant beside `DIFF_DISPLAY_MIME` in
   `crates/pa-core/src/kernel/shared.rs:59`:
   `application/vnd.prime-agent.context-fold+json`.
3. `dispatch_display` (`crates/pa-core/src/kernel/manager/events.rs:197`)
   buffers it on the active execution; `execution.rs:230` lifts it into the
   kernel `ExecuteResult`.
4. `convert_execute_result` (`session_engine/runtime_wiring.rs`) and
   `execute_ipython` (`tools/ipython.rs`) write it as `details.contextFold`.
   The kernel layer carries the payload as a raw JSON value; only
   `context_fold.rs` decodes it.

The marker payload, pinned once because four consumers read it (the view,
the handlers' open-fold scan, the compaction trigger and compactor, and
telemetry):

- begin: `{"kind": "begin", "description", "goal"}`
- end: `{"kind": "end", "description", "result", "folded", "messages",
  "tokens", "overBudget"}`

The host computes `folded` and the stats when `fold.end` runs and returns
them; the skill emits them unchanged. `fold.end` *always* emits an end
marker, `folded: false` included, so a voided fold stops counting as open
for the handlers; the view recomputes the pair either way and never trusts
`folded` for what it drops.

`details` is copied into the loop's `ToolResultMessage`
(`crates/pa-agent/src/agent_loop/tool_call.rs:312`), persisted in the session
JSONL, and rebuilt on resume, so the markers are visible to every later
request without any extra row.

### Host state is derived, never held

`fold.begin`/`fold.end` decide "is a fold open" by scanning the live
`agent.state().messages` for the last begin marker without a matching end
(the handlers already reach the agent through `requests.bound()`, as
`compact.run` does in `turn_boundary.rs`). No in-memory flag, so a resume or
a cell that dies between the host's OK and the `emit` cannot disagree with
the log.

Consequences to handle explicitly:
- A cell's own marker is not in `messages` until the cell settles, so two
  calls in one cell are invisible to the scan. The skill guards that itself
  with a per-cell flag keyed on the running cell id, and refuses a
  same-cell `begin` + `end` (folding one cell saves nothing).
- After a compaction summarizes an open fold's `begin` away, `fold.end`
  finds no open fold and answers `{"folded": false, "reason": ...}` (not an
  exception), naming compaction as the likely cause.
- Calls from a detached task with no running cell are refused (the skill
  checks `repl.current_cell_completion_context()`).
- `fold.end` in the same assistant turn as `fold.begin` (two cells of one
  assistant message) is refused: nothing lies between them to fold. While
  `fold.end` runs, its own assistant message is already the last assistant
  message in `agent.state().messages` (pushed at `MessageEnd`,
  `pa-agent/src/agent.rs:362`, before its tools run; a sequential batch
  pushes each sibling's result as it finishes, `agent_loop/tools.rs:125-127`).
  So the span to hide is everything after the begin group up to that last
  assistant message, and "same turn" means the begin group *is* that message.

The three refusals the handlers answer, as `{"folded": false, "reason"}`
or an error the skill raises: a nested `begin`; an `end` with no open
`begin` (most likely summarized away by compaction); an `end` in the same
assistant turn as its `begin`.

New module `crates/pa-core/src/session_engine/context_fold.rs` beside
`compact_carry.rs` holds the marker codec, the view, the open-fold scan and
the handler registration; `turn_boundary.rs` (769 lines) does not grow.

### The view: a pure function over the message list

`context_fold::fold_view(messages: Vec<AgentMessage>) -> Vec<AgentMessage>`.
The session log is never changed. Rules:

1. Pair each begin marker with the next end marker.
2. Drop every message strictly after the begin cell's *assistant group*
   (its assistant message and all of that message's tool results; an
   assistant turn may carry the `ipython` call plus a `bash` call, since
   `ipython` is `ExecutionMode::Sequential`, `tools/ipython.rs:946`, but the
   turn still holds several calls) and strictly before the end cell's
   assistant message. Only whole groups are dropped, so no `tool_use` ever
   loses its `tool_result`. `pa-ai`'s synthetic "No result provided"
   insertion (`providers/transform_messages.rs:174`) is a safety net, not
   something to rely on.
3. **Void, don't fold, across new input.** If the range holds a user
   prompt, a steering or follow-up message, a harness digest (delivered only
   at cold boundaries: start, resume, compaction head), a compaction summary,
   or a `goal_context` row of kind `budget_limit` or `objective_updated`,
   the pair is void and nothing is dropped. Rows that never reach the model
   do not void and are kept in place: loop custom rows that
   `loop_convert_to_llm` (`session_engine/messages.rs:393`) turns into
   nothing, such as the provider-retry outcome row a retry can append
   mid-fold. Decide that by calling `loop_convert_to_llm` on the single
   row, not with a second hand-written list. So a custom row gets two
   tests: (a) does it convert to anything? If not, keep it and do not void.
   (b) If it does, is it a `goal_context` row with `details.kind ==
   "continuation"`? Keep it and do not void (the decision below). Anything
   else voids. This is the whole of F1b; there
   is nothing to add at prompt admission in `pa-daemon` or `pa-cli`.
   `fold.end` reports a voided fold as `{"folded": false, "reason": "new
   input arrived during the fold"}`.

   **Decision: a fold may span goal continuations.** `goal_context` rows of
   kind `continuation` (`crates/pa-core/src/goals.rs:271`) are
   self-contained "keep going" nudges that restate the objective and goal
   state; each one supersedes the last and none refers to specific earlier
   work. They are *kept in place* and do not void the fold: the view drops
   the assistant groups around them, so the model reads begin cell,
   continuation rows, end cell. Kept rows only ever follow a tool result or
   another user-role row, which providers already accept (steering does the
   same). Without this, folding could not save anything in goal mode, the
   long-horizon mode the evaluation targets. The view therefore removes
   only the model's own work, never a row the host or the user wrote.
4. An end without a begin (the begin was compacted away) or a begin
   without an end (still open) drops nothing.
5. Everything before the begin cell is untouched, so the prefix up to the
   begin cell is byte-identical before and after `fold.end`.

`fold.end` answers with what it hid (message count and a chars/4 token
estimate, `pa_types::usage::estimate_tokens` over each message's JSON, the
estimator compaction uses). The skill prints the end template with those
numbers, which stays in the end cell's output and is therefore fixed,
deterministic text in later requests.

A `maxFoldTokens` change mid-session flips the over-budget line on earlier
in-fold results once, costing one cache miss; acceptable.

### Where the view applies

- `engine.rs:730`: wrap `fold_view` in `instrument_transform_context` in
  place of `pass_through_transform()` **only when the setting is on**. When
  it is off the wiring stays exactly as today, so parity mode is untouched.
- `side_question.rs:126` clones `parent_state.messages` into its own
  `Agent`, which has no `transform_context`. Apply `fold_view` to the clone
  (when the setting is on) so a side question reuses the main thread's
  cached prefix instead of paying for the folded work again.
- Nowhere else in the request path. `convert_to_llm` and the providers see
  the folded list.

### Compaction trigger: almost nothing to do

`estimate_context_tokens` (`session_engine/compaction.rs:234`) anchors on the
last provider-reported usage and adds chars/4 for later messages, so
"applying the view in `auto_compaction_due`" would not change the number.
Usually it does not need to: when the loop continues after the `fold.end`
cell, the next request is built from the folded list, so its usage already
reflects the fold.

The stale case is a turn that ends on the `fold.end` tool result. That is
a normal path, not only abort or error: `engine.rs` wires
`should_stop_after_turn` to the queued-steering probe, and
`agent_loop/run.rs:135` then ends the run straight after tool execution, so
it happens whenever the user types during a long fold. The anchor is then the
assistant message that called `fold.end`, measured on the unfolded context,
so the check reads high and may compact early.

Fix it with a suppression rule inside `threshold_compaction_due`
(`compaction.rs:271`), mirroring the existing "the usage anchor must
postdate the latest compaction" rule at `compaction.rs:287`: when the latest
tool result carrying an end marker with `folded: true` is newer than the
usage anchor, return `false` and wait for a fresh usage. Do not subtract an
estimate of the dropped span: `threshold_compaction_due` takes session-type
messages (`auto_compaction_due` converts by JSON round-trip), the view takes
loop-type messages, and recomputing the span would duplicate the view's
rules on a second type. The suppression needs only the marker decode, which
works on the `Option<Value>` details both types carry. Waiting is safe: the
next request is the folded one, so it is smaller than the stale anchor, and
its usage re-arms the check.

It must live in `threshold_compaction_due`, not in `auto_compaction_due`,
because `pa-daemon/src/autonomous_continuation.rs:196` calls
`threshold_compaction_due` directly; the other callers
(`pa-daemon/src/auto_compaction.rs:54`, `pa-cli/src/print_goal.rs:475`,
`pa-cli/src/print_boundary/compaction_arms.rs:123`) go through
`auto_compaction_due` and get it for free.

### With compaction

Compaction reads `FileEntry` rows from the session log
(`compact_session.rs`, `structural_compaction::events::normalize`), not the
live messages, so the view never reaches it. That is correct: compaction
summarizes everything before the cut, folded or not.

What is missing is the fold's own result. The structural compactor keeps
typed `details` fields (status, `bashCommands`, `sentAgentMessages`, edited
paths; `events.rs:303`) and drops prose, so a fold's `result` line would be
lost. It already carries model-written text verbatim within bounds as
`blocks` (`state.rs:77`, `BlockOrigin::BranchSummary`). Add
`Event::FoldClosed { description, result }`, produced in `tool_end` for an
end marker with `folded: true`, and apply it as a `Block` with a new
`BlockOrigin::Fold` (bounded by `BLOCK_BYTES`, like a branch summary) plus a
`note` line. The LLM engine needs nothing: it reads the cells as text.

`STATE_VERSION` stays 2. The new variant is additive: states saved before
it still parse. An older binary reading a newer state fails the parse, and
`State::from_value` (`state.rs:216`) falls back to carrying the previous
summary as a labeled excerpt, which is the designed degradation. Bumping to
3 would push every existing session through that fallback once. Flag this
for the reviewer, since "version 2 added `carried`" is a precedent for
bumping on additive changes.

Dropped from the first draft: auto-close on compaction and on size with a
host-written summary row, and "compaction never cuts inside an open fold".
Rule 4 of the view makes any cut safe. Size is handled the paper's way, by
the over-budget reminder asking the model to close the fold itself, and
compaction is the backstop for a fold the model never closes.

### Gates (one load-time exclusion, plus handlers and transform)

- **Skill**: exclude it at resource load, exactly like the default-off
  websearch skill (`packages/resolve/auto.rs:144` pushes
  `-websearch/SKILL.md` into the built-in skill overrides). Push
  `-fold/SKILL.md` there unless `contextFolding.enabled`. An excluded skill
  never reaches `resources.skills`, so it is absent from the prompt's skills
  inventory, from `/skill:`, and from `kernel_python_skills`
  (`runtime_wiring.rs:194`), so the kernel neither installs nor pre-imports
  it. This replaces decision-api's three per-site filters (`engine.rs:462`,
  `:594`, and the session-skills filter); decision-api needs those because
  its gate is a model reference, not a plain switch. No pre-import filter
  composition and no refactor of the decision-api gate are needed.
- **Handlers**: like compact (`engine.rs:374`), `fold.begin`/`fold.end` are
  registered only when the setting is on, so a stale `fold` package left in
  the kernel venv from an earlier enabled session gets "host request type
  ... is not available in this session".
- **Transform**: pass-through unchanged when off (above).

Read the setting through one `SettingsManager` getter used at both
call sites (the package resolver and `engine.rs`), so neither is a
single-use helper (AGENTS.md).

`skills/fold/` (SKILL.md, pyproject, `src/fold`, `tests/`) is packaged like
every bundled skill (`scripts/package_release.py` ships the whole `skills/`
tree). Two guard tests in `crates/pa-core/tests/prompt_guards.rs` must
change with it: add `"fold"` to `TS_PACKAGED_SKILL_SET` (line ~499, with the
same "Experimental, no TS counterpart yet" note as `decision-api`) and to
`gated_bundled_skills()` (line 348). The second keeps
`core_layer_documents_the_real_tool_surface` from demanding that the static
core layer document `fold.begin`/`fold.end`; it maps each registered
request to its module by prefix, so the requests must be named `fold.*` to
match the import name `fold`.

All hosts go through `pa-core`'s `create_session` (`engine.rs:196`): daemon
workers (`pa-daemon/src/worker/input.rs`, `agent_engine/lifecycle.rs`), RLM
children (`rlm_children/host.rs`) and CLI print mode (`pa-cli/src/print_runtime.rs`,
`print_goal.rs`). Wiring in `engine.rs` covers all of them, children
included.

Setting: a typed top-level `contextFolding: { enabled, maxFoldTokens }`
field in `crates/pa-core/src/settings/types.rs` beside `repeat_guard` (line
396), so a settings save keeps it; `enabled` defaults to off,
`maxFoldTokens` to 32,768. No `/settings` tab entry and no slash
command in F1: both are TUI changes that would need parity-diff evidence.

### Skill guidance (`skills/fold/SKILL.md`)

The surface contract keeps the static system prompt unchanged, so the
paper's MAIN/BRANCH role text goes into the skill, and its inventory
description must say *when* to fold, because untrained models rarely fold
on their own (main-thread length barely dropped before RL, Table 2). Adapt
the paper's coding prompt:
- Fold work that is trial-and-error or exploration: searching the codebase,
  writing a reproduction or test, an implementation attempt, a review pass,
  running and fixing tests, verification.
- Keep planning, decisions and the final review outside folds.
- One fold at a time; give each a goal that says what to bring back.
- The result must be self-sufficient, because the folded work is no longer
  visible: outcome, files changed, environment changes, key findings,
  assumptions, open questions.
- Kernel variables survive a fold; the conversation does not. Leave anything
  the next step needs in variables or files, and say so in the result.

### TUI and parity

F1 makes no TUI change. The ipython card shows `await fold.begin(...)` as
ordinary code, and the transcript still shows everything (folding affects
only what the model is sent). Add one test that the ipython card renders
identically with and without `details.contextFold`, as PR 0 did for
`details.diffs`. Add the divergence to `next-steps/09-rust-divergence-signoff.md`
and name it in the commit message.

### Telemetry

Count in the telemetry subscriber, not in the handlers and not in the
view. The view runs on every request; the handlers cannot reach the
telemetry state, because `SessionTelemetry` is built at `engine.rs:973`,
after the host handlers are registered at `:374`. The subscriber already
receives every agent event: its `ToolExecutionEnd` arm
(`session_engine/telemetry.rs:1141`) binds the event with `..` today and
ignores `result`. Bind `result`, decode `details.contextFold`, and:

- count into `SessionTotals` (`telemetry.rs:191`, beside the `cache_miss_*`
  fields): `context_fold_begin_count`, `context_fold_closed_count`,
  `context_fold_voided_count`, `context_fold_tokens_folded` (sum of the end
  markers' `tokens`), `context_fold_budget_nudge_count` (end markers with
  `overBudget`), plus their schema entries in
  `crates/pa-telemetry/src/catalog.rs` (~line 700). Primitives only;
- on an end marker with `folded: true`, reset the cache-miss baseline
  (`state.prev_request = None`), exactly as `note_compaction` does
  (`telemetry.rs:698`).

Why the reset: the cache-miss attribution (`session_engine/cache_miss.rs`)
fingerprints only the model, tools and system prompt (`cache_miss.rs:186`),
so the view's reminder lines cannot skew it, but it takes the previous
request's prompt size as the cacheable baseline. A closed fold shrinks the
prompt by design, so without the reset every `fold.end` would count as an
`unattributed` miss and the F2 decision would read noise. The
`ToolExecutionEnd` event fires before the next request is built, so the
reset is in time. With the subscriber doing all of this, `pa-daemon` and
`pa-cli` need no edits.

### Tests

- The view (pure, fast): prefix up to the begin cell byte-identical after
  `fold.end`; whole assistant groups dropped, including a turn that mixes
  `ipython` and `bash` calls; voided across each voiding row kind; a fold
  spanning `continuation` goal rows keeps those rows in place and still
  drops the work around them; orphan end and open begin drop nothing; two
  sequential folds.
- Handlers: nesting refused; `end` without `begin` refused; the
  compacted-away begin answers `folded: false` with the reason.
- Skill (Python, `prime-agent-runtime/test` style): same-cell begin + end
  refused; calls outside a cell refused; `goal` required; the marker is
  emitted only after the host's OK; the end result uses the template.
- Reminder in the view: no line without an open fold; the line on every
  tool result after the begin cell (not on the begin or end cell); the
  over-budget line past `maxFoldTokens`; a result's bytes are identical
  across successive requests and across a resume; a voided fold keeps its
  earlier lines; the session log and the emitted tool result never carry
  the line.
- Void rule: a provider-retry outcome row inside the span does not void the
  fold; a steering user message does.
- Cache-miss attribution: the request after a closed fold classifies no miss
  (the baseline was reset); a voided fold does not reset it.
- Kernel path: a cell that emits the fold MIME settles with
  `details.contextFold` (mirror `cell_edit_diffs_are_saved_in_the_result_details`).
- Resume: an open fold survives a session reload (the scan works over
  `build_session_context` output) and can be closed afterwards.
- Compaction: the end marker's result appears in the deterministic summary.
- Trigger: a turn ending on the `fold.end` result (stopped by queued
  steering, and by abort) does not compact early.
- Setting off: the transform is the pass-through and the skill is absent
  from the prompt.

## F2: cache breakpoint at the fold start (defer until measured)

Lower value than the first draft assumed. `anthropic/convert.rs:231` puts
`cache_control` on the last user message, so the request made right after
the `begin` cell already wrote a cache entry ending exactly at that cell. On
the request after `fold.end`, the cache lookback from the new breakpoint
reaches that entry, because the kept begin-to-end segment is only a few
blocks. (Verify against Anthropic's current lookback limit in the
provider docs while implementing.) The gap is the TTL: a fold that runs
longer than the cache retention lets that entry expire, because the requests
inside the fold hit longer prefixes instead.

So build F2 only if the cache-miss attribution shows misses after long
folds. If it is built, do not teach `pa-ai` about folds: the marker is in
`ToolResultMessage.details`, which does reach `pa-ai`
(`pa-types/src/ai/mod.rs:544`), but the provider crate should take a generic
"extra breakpoint here" hint that `pa-core` sets, not parse
`details.contextFold`.

OpenAI's automatic prefix caching needs nothing. The Codex websocket
transport (`openai_codex_responses/websocket.rs:555`) only sends
`previous_response_id` when the new input strictly extends the previous one,
so a fold costs one full resend and stays correct.

## F3: `fold.consolidate` (1-2 days, low risk)

The model marks closed folds to merge; they are applied at the next
compaction (`skills/fold` plus the compactor's fold event from F1). Still
optional; decide after the evaluation.

## Evaluation (3-5 days)

Run an A/B comparison on long-horizon tasks: engine (`llm` versus
`deterministic`) and folding on or off. Measure success rate, tokens,
compactions per task and cache-hit ratio, plus the paper's two behaviour
measures: main-thread length (tokens the model is sent at the end of a task,
the paper's "Main Len") and how often folds are voided or hit the budget.
The paper's prompt-only numbers set the bar: folding matched compaction and
lost to a long context, so the result to beat is "same success rate, fewer
tokens". A drop in success rate keeps folding off. Use it to decide whether folding
becomes a default, and whether the deterministic engine stays the default.
Use the zro provider for the real-model runs. The bench harness
(`scripts/bench`) already does paired before/after runs.

## Effort

F1 as revised: the skill and kernel channel (four small edits mirroring PR
0), the `context_fold` module with the view (including the reminder and
budget lines), scan and handlers, the settings field, the load-time skill
gate, the side-question line, the trigger suppression, the compactor block,
the skill guidance, telemetry in the subscriber, and tests. About 2-3 days
with an agent (mostly `make check` cycles), a week or more by hand; the
per-step breakdown is in `05b-context-folding-build-plan.md`. F1b is folded
into F1's view. F2 is deferred, F3 is optional, and the evaluation does not
get shorter.

## Gates

- New REPL calls change the surface the model sees (`AGENTS.md`, "Surface
  contract"). Document the API only in the gated skill and leave the static
  system prompt unchanged.
- Record the divergence from TS in each commit message and in
  `09-rust-divergence-signoff.md`.

## What changed from the first draft

- Marker channel made concrete: host requests carry no cell id, so markers
  go through the per-cell display MIME path, not the host handler.
- Open-fold state is derived from the live messages, not held in memory.
- F1b (auto-close on new input) became a rule of the view: a fold never
  spans new input. No admission changes in `pa-daemon` or `pa-cli`. Goal
  `continuation` rows are the one exception, kept in place so folds work in
  goal mode.
- Auto-close on compaction/size with a host-written summary row, and "never
  cut inside an open fold", were dropped: an orphaned marker drops nothing,
  an over-budget fold is asked to close itself, and compaction is the
  backstop.
- "Apply the view in `auto_compaction_due`" was wrong: the estimate anchors
  on provider-reported usage. Only a turn ending on the `fold.end` result
  (queued steering, abort) needs a correction, inside
  `threshold_compaction_due`.
- Compaction needs a typed fold event, because the structural compactor
  drops prose and would lose the fold's result.
- Side questions need the view applied to their cloned messages.
- F2 deferred: the existing last-message breakpoint already covers short
  folds.
- From the paper and its code: `fold.begin` takes a required `goal`; `begin`
  returns role-change instructions and `end` renders a fixed template; a
  reminder line on every tool result while a fold is open; a per-fold token
  budget that asks the model to close with progress and pending work
  (replacing the host-written auto-close summary); MAIN/BRANCH guidance in
  the skill; and expectations set by the paper's prompt-only numbers.
- Second code pass: the reminder and budget lines live in the view, not in
  `after_tool_call`, because a stored line would show in the TUI cards and
  the session log; `fold.begin`/`fold.end` print instead of returning text;
  the void rule ignores rows `loop_convert_to_llm` never sends; wiring in
  `create_session` covers the daemon, print mode and RLM children.
- Third code pass (with the build plan, `05b-context-folding-build-plan.md`):
  the marker payload is pinned and `fold.end` always emits an end marker;
  the trigger fix is a suppression rule, not a subtraction, so the span
  rules exist only once; the skill gate is the websearch-style load-time
  exclusion, not decision-api's three filters; telemetry and the
  cache-miss baseline reset live in the telemetry subscriber, because the
  handlers are registered before `SessionTelemetry` exists; the compactor
  carries a closed fold's result as a `BlockOrigin::Fold` block without a
  state-version bump; two prompt-guard tests change with the new skill.
