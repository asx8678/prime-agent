# 10. Orchestration patterns: what fits prime-agent, and how to build it

Saved 2026-10-12. Source: pi-fabric 0.104.1 as installed for `pi`
(`~/.pi/agent/npm/node_modules/pi-fabric/skillsets/python/`, 15 skills,
markdown only) and borrow item 46. Every prime-agent anchor below was opened
in this pass.

## The verdict

Fabric's skills are written against its own `agents.*` host API
(run/spawn/wait, persistent actors, lifecycle subscriptions, mesh,
decisions), so none of them can be copied over. What carries over is each
pattern's semantics. Sort them by which prime-agent mechanism already owns
that kind of work:

| Pattern (fabric skill) | Fit | Where it belongs |
|---|---|---|
| council: 3-5 roles in parallel, then a synthesis | **Build now** | A Python skill function on `rlm.spawn(schema=)` + `rlm.collect` |
| fusion: several models answer, a judge compares | **Build now** | The same skill, with `rlm.find_models` + `spawn(model=)` |
| workflow: discover, fan out, verify | Already there | The factory: `review-sweep` is exactly this shape |
| rlm: work too big for one context | Already there | `rlm.spawn` itself |
| advisor / supervisor / spec: observers that react to the main session | **Not now** (phase 2) | Needs two primitives prime lacks (below) |
| swarm, graph: durable actors, mesh, human decision steps | Does not fit | Fabric runtime substrate, not a pattern |
| schema: transactional, evidence-gated file changes | Does not fit | Fabric runtime substrate |
| jev, foreman: typed judgments, QuickJS programs | Does not fit | The TypeSafe/QuickJS runtime; `decision-api` is prime's own System 1 lane |
| exec, guide | Does not fit | `exec` is fabric's code tool (prime's `ipython` already is that); `guide` only routes among fabric skills |

### Why council and fusion are a skill, not factory machines

- `rlm.factory.run(spec_id, *, name=None)` takes **no inputs**
  (`rlm/factory.py:1524`). A machine's prompts are fixed when it is written,
  so a reusable "council on this question" cannot be a library machine; the
  model would have to author a new machine per question.
- `run` returns immediately and the run continues in a background task, so a
  council could not hand its synthesis back to the calling cell.
- The factory ships disabled (`/factory on`).

Generating a machine on the fly (`create_factory` + `run`) would buy the TUI
diagram and run budgets, but inherits all three problems. Rejected.

### Why the observers wait

Prime has most of the parts: a child can read the parent's recent messages
(`agent_observe.recent_messages`), message it (`agent_message.send`), and wake
itself on a timer (`rlm_heartbeat.create`, on its own session). Two
primitives are missing:

1. **No event trigger.** Children cannot subscribe to the parent's
   lifecycle (settled, tool error). The daemon's `turn_done` watch
   (`pa-daemon/src/rlm_children.rs:426`) only delays a child's first prompt.
   A heartbeat-polling observer spends a child model turn on every tick,
   working or not.
2. **No non-waking delivery.** Messages to the parent always steer: they
   interrupt a busy turn and wake an idle parent. Fabric's advisor relies on
   advice that waits for the next turn without forcing one.

Goal mode (`/goal`, `goal.complete`) already covers "keep working toward an
objective" with the same model. Revisit observers after the council skill is
measured: either a daemon "parent settled" subscription (new wire surface,
parity sign-off) or a cheap-model heartbeat supervisor.

## How to build it

### Facts that shape the API (checked)

- **A cell may block for the whole council.** No cell execution limit exists
  in `kernel/shared.rs` or `tools/ipython.rs`, and `rlm.collect`'s
  `timeout_ms` caps at `2_147_483_647` (`rlm_host.rs:33`). `council()` can be
  one blocking call that returns the synthesis, like fabric's `agents.run`.
- **`/skill:<name> <args>` passes the arguments.** They ride after the skill
  block as `user_message` (`pa-types/src/skill_blocks.rs`), so
  `/skill:orchestrate council <question>` hands the model the question.
- **User-only skills are supported** and still reach the kernel.
  `disable-model-invocation: true` keeps a skill out of the prompt inventory
  (`prompts/system_prompt.rs:175`), but `kernel_python_skills` installs and
  pre-imports every skill (`runtime_wiring.rs:194`).
- **Nothing caps how many children run at once.** Only the `rlmMaxDepth`
  setting (depth) and the cost ceiling's `spawn_limit` exist. The skill has to cap
  breadth itself.
- **Children cannot be read-only.** The write policy is on hold
  (`07-rlm-api-trio-decision.md` §3); "do not edit files" is prompt-level
  only. Say so in SKILL.md.
- **`skill_use_count` is generic** (`admission.rs:111`). It does not tell
  which skill ran, so it does not meet AGENTS.md's per-feature telemetry
  rule.

### The package: `skills/orchestrate/`

User-invoked, gated, shaped exactly like `skills/fold/` (pyproject,
SKILL.md, `src/orchestrate/__init__.py`, `tests/` with a fake `rlm`). No
"fabric" or "pi" naming in anything shipped (README branding rule).

```python
await orchestrate.council(task, roles, *, schema=None, model=None, timeout_s=900)
await orchestrate.fusion(task, models, *, judge=None, timeout_s=900)
```

Port fabric's council semantics, not just its shape:

- 3-5 distinct, non-empty roles (deduplicated); fusion takes 2-4 models,
  resolved through `rlm.find_models`. Anything else raises before a spawn.
- Every reviewer is a child: `rlm.spawn(prompt, name=..., schema=REPORT)`.
  `REPORT` is a small JSON Schema (summary, findings, recommendation), so
  `schema_valid` names the role that answered badly. An answer that fails
  the schema is kept as unstructured text, not dropped.
- One `rlm.collect(handles, timeout_ms=...)`. Each role ends as completed,
  failed (child error, spawn refused by a cost ceiling) or timed out.
  Failures stay isolated per role.
- Synthesis is one more child, and it reads only the completed reports.
  It is skipped when only one role completed (another agent adds no
  diversity), and the raw reports are returned only when synthesis fails.
- The result is a dict: `status` (`success`/`partial`/`failed`),
  `coverage {requested, completed}`, `failures`, `result`, and
  `synthesisSkipped` or `synthesisError` when they apply. Nothing reruns
  automatically.
- Child names carry a short per-call run id (`council-3fa2c1-security`):
  sibling names must be unique, and two councils in one session must not
  collide.
- Cleanup: after collecting, delete every child the call spawned
  (`rlm.delete_subagent`; their session files stay on disk). Children still
  running at the timeout are deleted too, which stops them. On cancellation
  (`CancelledError`), delete whatever was spawned, then re-raise.
- Cost: the docstring and SKILL.md tell the user to bound spend with
  `rlm.set_cost_ceiling(...)`; the skill never raises a ceiling.

`SKILL.md` frontmatter: `disable-model-invocation: true`, and a description
that says what each pattern is for. The body covers when to use council
(architecture choices, plans, adversarial review, never a lookup) versus
fusion (comparing models), the read-only caveat, and cost.

### Gate, telemetry, guards

- **Setting:** `orchestration.enabled` (default off). The resolver excludes
  `-orchestrate/SKILL.md` through `gated_bundled_skill_overrides`
  (`packages/manager.rs`, added for `fold`), so with the setting off nothing
  changes, including the TUI's `/skill:` list.
- **Telemetry:** a host request `orchestrate.note {pattern, requested,
  completed, failed, synthesis}`, registered only when enabled, counting into
  `SessionCounters` (the `compact_carry` precedent: the counters exist when
  handlers register, `SessionTelemetry` does not). New optional
  `agent session ended` properties: `orchestrate_council_count`,
  `orchestrate_fusion_count`, `orchestrate_roles_completed`,
  `orchestrate_roles_failed`, `orchestrate_synthesis_skipped_count`.
  Primitives only.
- **Guards:** add `"orchestrate"` to `TS_PACKAGED_SKILL_SET` (experimental,
  no TS counterpart) and `gated_bundled_skills()` in
  `crates/pa-core/tests/prompt_guards.rs`; a `python.yml` step; a
  `.changes/` entry; a row in `09-rust-divergence-signoff.md`.

### Tests

- Python (`skills/orchestrate/tests/`, fake `rlm`): role validation and
  deduplication; a role failure isolated from the others; synthesis skipped
  at one completed role; raw reports returned when synthesis fails; timeout
  deletes the running children; cancellation deletes what was spawned;
  unique names across two calls; schema-invalid answers kept as text; the
  telemetry note sent once per call.
- Rust: the resolver gate test, beside the fold one; the handler counting
  test; the telemetry property test.
- Daemon end to end (`pa-daemon/tests/rlm_children_e2e.rs` harness,
  `scripted/faux-1`): a 3-role council over scripted children returns
  `status: success` with a synthesis, and leaves no children behind.
- Real model: a zro run in a throwaway agent dir (never the user's
  `~/.prime/agent`; see the realcheck scripts), recording requests, as for
  context folding.

### Evidence of value

None of this shows that a council beats a single strong answer. Measuring
that needs bench tasks with checks, the same gap as context folding's step 8
(`scripts/bench/` has one smoke task). Don't claim a benefit before that
run.

## Size

Python about 250 lines plus tests; Rust about 150 lines (handler, counters,
catalog, gate, guards) plus tests; one daemon e2e test. Roughly one session
of work, then the zro check.
