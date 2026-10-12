# 5b. Context folding: build plan

Saved 2026-10-12. The design is `05-context-folding.md`; this file is the
order of work: what each commit contains, the files it touches, the tests
that prove it, and how to verify it. Every `file:line` below was checked
against `main` at `bc12193bc`; re-check line numbers before editing, since
another session pushes to `main`.

## How to work through it

- Work on `main` in this checkout, one commit per step, no branches. Stage
  only the paths a step lists (never `git add -A`); run `git fetch && git
  status` before each commit, because another session shares the checkout.
- Every commit must pass `make check` (fmt, clippy at `-D warnings`, the
  workspace tests, the release build). Compare test failures with
  `known-macos-failures.txt`; a name not listed there is new. Clippy's
  dead-code lint is why each step wires what it adds: a `pub(crate)`
  function used only by tests fails the build.
- Push only when steps 1-6 are all green, and only after the owner says so.
  They go out together because `AGENTS.md` wants a feature's telemetry in
  the same change as the feature.
- Commit messages: no identity overrides, no `Co-authored-by`. Each message
  states the TS divergence ("Rust-only, default off; with
  `contextFolding.enabled` unset nothing the model or user sees changes"),
  ownership compliance (pa-core internals plus the `skills/fold` package; no
  new crate dependency; no `pub use`), and the change class (internals; the
  settings field and the Python API are new surface, both gated).

## The six decisions

| # | Question | Decision | Step |
|---|---|---|---|
| 1 | Where does the "fold is open" reminder go? | In the per-request view, never in the stored tool result. A stored line would reach the TUI (the merged result is what `emit_tool_execution_end` sends, `agent_loop/tool_call.rs:250-290`) and the session log. A result gets the line when a begin precedes it with no end or voiding row in between. That depends only on earlier messages, so the bytes never change between requests. | 2 |
| 2 | How do `fold.begin`/`fold.end` show text to the model? | They `print()` and return `None`. The model sees stdout, stderr and the `repr` of the last expression (`repl.py:676`); display `text/plain` never reaches it. | 5 |
| 3 | Which rows cancel (void) an open fold? | Two tests per non-assistant row. (a) Does `loop_convert_to_llm` (`messages.rs:393`) turn it into anything? If not, it is bookkeeping: keep it, no void. (b) If it does, is it a `goal_context` row with `details.kind == "continuation"`? Keep it, no void. Anything else voids (user prompts, steering, `!` commands, digests, budget-limit and objective-updated goal rows). `fold.end` always emits an end marker, `folded: false` included, so a voided fold stops counting as open. | 2, 5 |
| 4 | How is the skill hidden while the setting is off? | The websearch way: push `-fold/SKILL.md` into the built-in skill overrides in the package resolver (`packages/resolve/auto.rs:144`). An excluded skill never loads, so it is absent from the prompt, from `/skill:`, from the kernel install and pre-import, and from the `prime-agent prompt` dump. The dump (`pa-cli/src/prompt_command.rs:107`) loads resources without passing settings, and `load_resources` then reads them from disk (`resources/mod.rs:127-130`), so the dump sees the real setting. Decision-api's four separate filters are not needed. Two guard tests change with the skill. | 5 |
| 5 | Where is it wired? | Once, in `create_session` (`engine.rs:196`), which every host uses: daemon workers, CLI print mode, RLM children. No edits in `pa-daemon` or `pa-cli`. | 2, 5 |
| 6 | Where do telemetry and the cache-miss baseline reset live? | In the telemetry subscriber's `ToolExecutionEnd` arm (`telemetry.rs:1141`), which already sees every tool result. Not in the handlers: they are registered at `engine.rs:374`, before `SessionTelemetry` exists (`engine.rs:973`). On an end marker with `folded: true` the subscriber sets `prev_request = None`, as `note_compaction` does (`telemetry.rs:698`). | 4 |

## Fixed definitions

Use these names and texts everywhere; the tests pin them.

**Names.** Settings key `contextFolding` with `enabled` (default `false`)
and `maxFoldTokens` (default `32768`). Skill `fold` (import name `fold`).
Host requests `fold.begin` and `fold.end` (the `fold.` prefix must match
the import name, see step 5). Display MIME
`application/vnd.prime-agent.context-fold+json`, constant
`CONTEXT_FOLD_DISPLAY_MIME`. Tool-result details key `contextFold`.

**Marker payload.**
- begin: `{"kind": "begin", "description": str, "goal": str}`
- end: `{"kind": "end", "description": str, "result": str, "folded": bool,
  "messages": int, "tokens": int, "overBudget": bool}`

**Module layout.** `crates/pa-core/src/session_engine/context_fold/`:
- `mod.rs`: the marker codec (`ContextFoldMarker`, decoded from
  `Option<&Value>` so it serves loop and session message types alike), the
  open-fold scan, and the span check (which messages a begin/end pair hides,
  or why it is void). The view and the handlers share the span check and the
  scan, so neither is a single-use helper.
- `view.rs`: `fold_view(messages, max_fold_tokens)`, used by the transform
  and by side questions.
- `handlers.rs`: registration of `fold.begin`/`fold.end` on the
  turn-boundary requests (`TurnBoundaryRequests::bound()` is public,
  `turn_boundary.rs:127`), so `turn_boundary.rs` (769 lines) does not grow.
- `tests.rs`.

**Texts.**
- `fold.begin` prints: `Fold opened: {description}. Work only on this goal.
  Finish with await fold.end(result), where result states the outcome,
  files changed or created, environment changes, key findings, assumptions
  and open questions. Everything until fold.end will be hidden from later
  turns; only the result stays. Keep anything you still need in kernel
  variables or files.`
- `fold.end` prints when folded: `Fold closed: {description} ({messages}
  messages, ~{tokens} tokens hidden from later turns).\nResult:\n{result}`.
  When not folded it prints the reason.
- Reminder appended by the view: `[fold open: {description}] Work only on
  this fold's goal; when it is done, call await fold.end(result).`
- Over-budget line, past `maxFoldTokens`: `[fold over budget:
  {description}, ~{tokens} tokens] Stop and call await fold.end(result) now,
  stating the progress made and the work still pending.`
- Refusals: nested begin ("a fold is already open: {description}; finish it
  with fold.end before starting another"); end with no open begin ("no fold
  is open; compaction may have summarized it"); end in the same assistant
  turn as its begin ("nothing to fold yet: call fold.end in a later cell").

## Step 1: kernel channel for the marker

Commit: `kernel: carry context-fold markers into the ipython tool-result details`

The PR 0 path (`016489755`) for `edit()` diffs, for one more MIME. Inert
until step 5 emits markers.

Changes:
- `crates/pa-core/src/kernel/shared.rs:59`: add `CONTEXT_FOLD_DISPLAY_MIME`
  beside `DIFF_DISPLAY_MIME`; add `context_fold: Option<Value>` to the
  kernel `ExecuteResult` (`shared.rs:200`). The kernel layer keeps the raw
  JSON object and never interprets it.
- `kernel/manager/mod.rs:88`: `context_fold: Option<Value>` in
  `ExecBuffers`; `None` in `InternalExecuteResult::aborted` (`mod.rs:137`).
- `kernel/manager/events.rs:197` (`dispatch_display`): store an object
  payload under the MIME; the last one in a cell wins (the skill emits at
  most one per cell).
- `kernel/manager/execution.rs:223`: move it into the result.
- `session_engine/runtime_wiring.rs:326` (`convert_execute_result`) and
  `tools/ipython.rs:46` (tool `ExecuteResult`): carry it through.
- `tools/ipython.rs:769-840`: `details["contextFold"] = marker` for every
  status, like `diffs`.
- The kernel `ExecuteResult` has no `Default`, so add `context_fold: None`
  to every literal: `kernel/manager/execution.rs` (2), `kernel/manager/mod.rs`
  (3), `session_engine/runtime_wiring.rs` (3), `tools/golden_replay.rs` (1),
  `tests/kernel_capture_freshness.rs`, `tests/kernel_lifecycle.rs`,
  `tests/kernel_restore_guards.rs` (1 each). The ipython tool's own struct
  derives `Default`.

Tests:
- `kernel/manager/events.rs` tests: `a_context_fold_payload_is_buffered_on_the_running_cell`.
- `runtime_wiring.rs` tests: `the_converted_result_keeps_the_context_fold_marker`.
- `tools/ipython.rs` tests: `cell_context_fold_marker_is_saved_in_the_result_details`
  (mirror `cell_edit_diffs_are_saved_in_the_result_details`, line 1663).
- `pa-tui/src/tool_card/ipython/tests.rs`: `the_card_renders_identically_with_and_without_context_fold_details`,
  the parity evidence that no TUI change happens.

Verify: `cargo test -p pa-core kernel::manager`, `cargo test -p pa-core
tools::ipython`, `cargo test -p pa-tui tool_card`, then `make check`.

## Step 2: settings, the view, and the transform

Commit: `session: hide closed context folds from model requests behind contextFolding.enabled`

Changes:
- `settings/types.rs` (beside `repeat_guard`, line 396):
  `pub context_folding: Option<ContextFoldingSettings>` with
  `enabled: Option<bool>` and `max_fold_tokens: Option<u64>`, camelCase,
  `skip_serializing_if` like `RepeatGuardSettings`. `settings/manager.rs`
  (beside `get_repeat_guard_enabled`, line 434):
  `get_context_folding_enabled()`, used here and by the resolver in step 5.
- `session_engine/context_fold/mod.rs` and `view.rs` as defined above. View
  rules (design doc, "The view"):
  1. Pair each begin marker with the next end marker.
  2. The begin group is the assistant message that issued the begin call
     plus all of its tool results; the end group likewise. Drop every
     message strictly after the begin group and strictly before the end
     group's assistant message. Only whole groups go, so no tool call loses
     its result.
  3. Void rule: decision 3. Rows that pass test (a) or (b) stay in place.
  4. An end without a begin, or a begin without an end, drops nothing.
  5. Reminders: decision 1. Add the line to every tool result after the
     begin group while the fold is open, or the over-budget line once the
     span's estimate (`pa_types::usage::estimate_tokens` over each message's
     JSON) passes `maxFoldTokens`. Never on the begin or end cell's own
     result. Matches stay exhaustive, with no wildcard arms (`AGENTS.md`).
- `engine.rs:728-731`: when enabled, wrap a transform that calls `fold_view`
  in `instrument_transform_context` instead of `pass_through_transform()`;
  when off, leave the line exactly as it is. Read the setting with the other
  captures before `settings` moves into the loader (`engine.rs:246-262`).
- `side_question.rs:126`: when enabled, apply `fold_view` to the cloned
  parent messages, so side questions reuse the main thread's cached prefix.

Tests (`context_fold/tests.rs`, pure and always run):
- `a_closed_fold_drops_the_work_between_its_begin_and_end_groups`
- `the_prefix_up_to_the_begin_group_is_byte_identical_after_the_fold_closes`
- `whole_assistant_groups_are_dropped_with_all_their_tool_results`
- `new_input_inside_the_span_voids_the_fold`, one case per voiding row
  kind: user prompt, steering, `!` command, harness digest, compaction
  summary, budget-limit goal row, objective-updated goal row
- `a_goal_continuation_row_inside_the_span_is_kept_and_does_not_void`
- `a_bookkeeping_row_inside_the_span_is_kept_and_does_not_void` (a
  custom row `loop_convert_to_llm` drops, such as the provider-retry outcome
  row, `create_provider_retry_outcome_message`, `messages.rs:116`)
- `an_unmatched_end_or_an_open_begin_drops_nothing`
- `two_sequential_folds_both_close`
- `in_fold_results_carry_the_reminder_but_the_begin_and_end_cells_do_not`
- `the_reminder_becomes_the_over_budget_line_past_max_fold_tokens`
- `a_result_keeps_the_same_bytes_as_the_conversation_grows`
- `a_voided_fold_keeps_its_earlier_reminder_lines`
- `the_view_is_the_identity_without_markers`

Engine test (beside the scripted-provider engine tests in
`session_engine/tests.rs`): `a_resumed_session_with_a_closed_fold_sends_the_folded_view_only_when_enabled`.
Seed a session log with a closed fold (marker details on the begin and end
results), run one prompt through `ScriptedProvider`, and compare
`calls()[0].messages` with the setting on and off. Side question:
`a_side_question_sees_the_folded_view` in `side_question.rs` tests.

Verify: `cargo test -p pa-core context_fold`, `cargo test -p pa-core
session_engine`, `make check`.

## Step 3: compaction trigger and the deterministic summary

Commit: `compaction: wait for fresh usage after a closed fold, and carry fold results in the deterministic summary`

Changes:
- `session_engine/compaction.rs:271` (`threshold_compaction_due`): mirror
  the compaction rule at line 287. If the newest tool result whose end
  marker has `folded: true` is newer than the usage anchor, return `false`.
  This fixes the turn that stops on the `fold.end` result (queued steering,
  `agent_loop/run.rs:135`; abort). It needs only the codec on session-type
  messages; the span rules are not duplicated.
- `structural_compaction/events.rs`: `Event::FoldClosed { description,
  result }`, produced in `tool_end` (line 279) from an end marker with
  `folded: true`, after the cell's `CellEnd`.
- `structural_compaction/state.rs:77`: `BlockOrigin::Fold`. Apply the event
  as a `Block` bounded by `BLOCK_BYTES` plus a `note` line, like
  `BranchSummary` (`state.rs:302`). Render its label in `render.rs`.
- `STATE_VERSION` stays 2: the variant is additive, earlier states still
  parse, and an older binary falls back to the labeled excerpt through
  `State::from_value` (`state.rs:216`). Put this in the commit message for
  the reviewer, since version 2 was bumped for an additive field.

Tests:
- `compaction.rs`: `threshold_compaction_waits_for_usage_newer_than_the_latest_closed_fold`,
  `a_voided_fold_does_not_hold_back_the_threshold`.
- `structural_compaction/tests.rs`: `a_closed_fold_result_is_carried_as_a_fold_block`,
  `an_open_or_voided_fold_adds_no_block`,
  `a_state_saved_before_fold_blocks_still_parses`.

Verify: `cargo test -p pa-core compaction`, `cargo test -p pa-core
structural_compaction`, `make check`.

## Step 4: telemetry

Commit: `telemetry: count context folds and reset the cache-miss baseline when one closes`

Changes:
- `session_engine/telemetry.rs:1141`: bind `result` in the
  `ToolExecutionEnd` arm and decode `details.contextFold`. Begin: count.
  End: count closed or voided, add `tokens`, count `overBudget`; when
  `folded`, set `state.prev_request = None`.
- `SessionTotals` (`telemetry.rs:191`): `context_fold_begin_count`,
  `context_fold_closed_count`, `context_fold_voided_count`,
  `context_fold_tokens_folded`, `context_fold_budget_nudge_count`, emitted
  where the `cache_miss_*` totals are.
- `crates/pa-telemetry/src/catalog.rs` (~line 700): the five optional
  properties (`count()`, and `tokens()` for the token sum). Primitives only.

Tests (`telemetry.rs` tests):
`fold_markers_count_into_the_session_totals`,
`a_closed_fold_resets_the_cache_miss_baseline` (the request after the
close classifies no miss), `a_voided_fold_keeps_the_cache_miss_baseline`,
plus whatever catalog test lists the emitted properties.

Verify: `cargo test -p pa-core telemetry`, `cargo test -p pa-telemetry`,
`make check`.

## Step 5: the skill, the handlers, and the gate

Commit: `skills: add the fold skill (fold.begin / fold.end), gated by contextFolding.enabled`

This is the commit that makes the feature reachable.

Changes:
- `skills/fold/`, shaped like `skills/compact/`:
  - `pyproject.toml` (hatchling, `packages = ["src/fold"]`,
    `requires-python = ">=3.14"`, no dependencies);
  - `SKILL.md`, whose frontmatter description says *when* to fold (the
    inventory line is all an untrained model sees up front), and whose body
    is the guidance from the design doc ("Skill guidance");
  - `src/fold/__init__.py`, with `async def begin(description: str, goal:
    str) -> None` and `async def end(result: str) -> None`. Each validates
    types, refuses outside a cell (`rlm.repl.current_cell_completion_context()`
    returns `None`) and refuses a second fold call in the same cell (a
    per-cell flag keyed on that context's `finished` event). It calls
    `host_request("fold.begin" | "fold.end", ...)`, emits the marker with
    `rlm.emit({CONTEXT_FOLD_DISPLAY_MIME: payload})` only after the host
    agrees (for `end`, also when the host reports a voided fold), then
    prints the text;
  - `tests/test_fold.py`.
- `session_engine/context_fold/handlers.rs`. Use `TurnBoundaryRequests::bound()`
  to reach `runtime.agent.state().messages`. Do the open-fold scan and the
  three refusals. Bound `description` (one line, 80 chars), `goal` (2,000
  chars) and `result` (16,384 chars). `fold.end` computes the span with the
  shared span check (it ends before the last assistant message, the
  in-flight one) and answers `{folded, reason?, description, messages,
  tokens, overBudget}`.
- `engine.rs:374`: register the handlers only when enabled, beside the
  compact handlers.
- `packages/resolve/auto.rs:144`: push `-fold/SKILL.md` unless
  `get_context_folding_enabled()`. The resolver reaches settings through
  `PackageManager` accessors (`packages/manager.rs:176`). Rather than add a
  second single-use accessor beside `bundled_websearch_enabled`, turn it into
  one accessor that returns the override patterns for the default-off
  bundled skills (websearch and fold). This touches websearch code inside the
  fold commit; if the reviewer prefers, split the reshaping into a
  behavior-preserving prep commit before step 5 (AGENTS.md change hygiene).
- `crates/pa-core/tests/prompt_guards.rs`: add `"fold"` to
  `TS_PACKAGED_SKILL_SET` (line ~499) with the comment "Experimental, no TS
  counterpart yet", and to `gated_bundled_skills()` (line 348) so
  `core_layer_documents_the_real_tool_surface` does not demand that the
  static core layer document it.
- `.github/workflows/python.yml` (beside the decision-api and edit steps,
  lines 34-40): run `skills/fold` tests with `uv run --no-project
  --managed-python --python 3.14 python -m unittest discover -s tests`.
- `.changes/context-folding.md`: what users get, the setting, default off.
- `next-steps/09-rust-divergence-signoff.md`: a new row. Folding writes no
  custom rows, so the "TS sends unknown custom rows to the model" hazard in
  that file does not apply. A Rust session with folds opened in TS shows
  the full, unfolded history. Note the new kernel API (gated) for the
  `07-rlm-api-trio-decision.md` question.

Tests:
- `context_fold/tests.rs` (handlers over a bound agent with synthetic
  messages): `begin_is_refused_while_a_fold_is_open`,
  `end_without_an_open_begin_answers_not_folded`,
  `end_in_the_same_assistant_turn_as_its_begin_is_refused`,
  `end_reports_the_hidden_span`, `end_reports_a_voided_fold`,
  `begin_and_end_arguments_are_required_and_bounded`.
- `skills/fold/tests/test_fold.py` (style of
  `prime-agent-runtime/test/test_agent_message_skill.py`, patching
  `host_request` and `emit`): `test_begin_prints_and_emits_only_after_the_host_agrees`,
  `test_a_second_fold_call_in_the_same_cell_is_refused`,
  `test_calls_outside_a_cell_are_refused`, `test_goal_is_required`,
  `test_end_prints_the_template_and_emits_the_host_stats`,
  `test_a_voided_end_emits_a_marker_and_prints_the_reason`,
  `test_a_refused_end_emits_nothing`.
- Resources: `the_fold_skill_loads_only_when_context_folding_is_enabled`,
  beside the websearch override tests in `packages/`.

Verify: `cargo test -p pa-core context_fold`, `cargo test -p pa-core --test
prompt_guards`, `cargo test -p pa-core packages`, the Python step from
`python.yml` run locally in `skills/fold`, then `make check`. Parity
evidence: run `prime-agent prompt` with the setting off before and after the
series; the diff must be empty.

## Step 6: end-to-end over a real kernel

Commit: `tests: context folding end to end over a real kernel`

`crates/pa-core/tests/context_fold_kernel.rs`, on the
`turn_boundary_kernel.rs` harness (`ScriptedProvider`, a real kernel,
skipped with a note when no kernel Python is installed;
`PA_CORE_KERNEL_PYTHON` points at one). Scripted turns: a cell that calls
`rlm.host_request("fold.begin", ...)` and emits the begin marker, two work
cells, a cell that ends the fold the same way, and a final answer. Calling
the bridge directly, as the existing harness does, tests the host side
without installing the skill into the kernel. Assert over
`ScriptedProvider::calls()`:
- the request after the end cell has neither work cell;
- the prefix up to the begin group is byte-identical between the request
  right after the begin cell and the one after the end cell;
- in-fold requests carry the reminder line, while the session log and the
  emitted tool results do not;
- the end cell's details carry `folded: true` and the stats.

Name: `a_closed_fold_is_hidden_from_the_next_request_over_a_real_kernel`.
Because it may skip on CI, the step 2 unit tests remain the always-run
verifier.

## Step 7: real-model check on zro

Commit: `next-steps: real-model check for context folding on zro`

`next-steps/tools/realcheck_fold.py`, built on `realcheck_zro.py`: a
throwaway agent dir holding only the zro provider (`glm-5.3-flash`),
`contextFolding.enabled: true`, synthetic prompts, `DO_NOT_TRACK=1`, never
the user's `~/.prime/agent`. Give it a multi-step task that invites folding
(explore a small generated repo, then fix a planted bug), and report:
- whether the model called `fold.begin`/`fold.end` unprompted, and whether
  it did with an explicit hint;
- end markers and their stats in the session JSONL;
- input-token usage before and after each close (it should drop);
- any provider error on the request after a close.

Open until proven: the folded shape drops whole assistant groups,
thinking blocks included, from inside a tool-use loop. The scripted test
proves the structure, not that providers accept it. Record which API
families this run covered (zro's `glm-5.3-flash` likely exercises neither
Anthropic extended thinking nor OpenAI Responses reasoning items). Keep
"verified on the APIs users run" as an unchecked evaluation gate until a
run covers them.

## Step 8: evaluation and the default

No code. Use the bench harness (`scripts/bench/README.md`) with one binary
and two configs that differ only in an agent dir whose `settings.json` sets
`contextFolding.enabled` on or off. Real runs on zro, long-horizon tasks.
Measure success rate, total and main-thread tokens, compactions per task,
cache-hit ratio, and the fold counters from step 4. The paper's prompt-only
numbers set the bar: folding matched compaction and lost to a long context,
so the target is the same success rate with fewer tokens. A drop in success
keeps the default off. Then decide F2 (cache breakpoint), F3
(`fold.consolidate`), and the goal-mode continuation nudge, as the design
doc describes.

## Effort

With an agent: steps 1-4 about a day, step 5 about half a day, steps 6-7 a
few hours, all bounded mostly by `make check` cycles on this machine. The
evaluation is days of wall time and real-model cost, and it does not get
shorter.

## Not in this plan

TUI or `/settings` changes, a slash command, F2, F3, and any change to the
static system prompt. Each would need its own parity evidence and sign-off.
