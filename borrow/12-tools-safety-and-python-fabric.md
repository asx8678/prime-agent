# Pass 12: the tool layer, the safety kernels, and the Python fabric lane

Twelfth pass, 2026-10-11 — the last structural layers: TS's native tool implementations vs the Python kernel, codex's two safety kernels (Guardian reviewer + execpolicy engine), and fabric's Python tree as the prime-agent compatibility lane. Extends passes [02](02-pi-and-fabric-deep-dive.md)–[11](11-testing-and-sizing.md). Nothing here is implemented.

---

## AB. The tool layer — TS `core/tools/` vs the Python kernel

TS implements its native tools in `pi/packages/coding-agent/src/core/tools/` (`bash`, `edit` + **`edit-diff`**, **`file-mutation-queue`**, `find`, `grep`, `ls`, `powershell`, `read`, `write`, `truncate`, **`output-accumulator`**, renderers, `tool-definition-wrapper`). Prime-agent splits differently: bash/edit are native agent tools, the rest run through the Python kernel (`rlm/`: bash.py, repl.py, machines/, factory.py, harness.py, mcp.py, skill.py).

The audit already verified the shared policy (truncation, ANSI, output files) matches; the genuine gap is **`file-mutation-queue.ts`**: per-file async mutation queues keyed by **realpath** (ENOENT falls back to the resolved path) so concurrent operations — background bash, parallel cells, subagents — can never interleave writes to the same file, plus a registration queue for setup. ~60 lines, high safety value for prime-agent's concurrency surface. (`edit-diff` rides the existing Python edit skill; `output-accumulator` overlaps Rust's truncate/render path.)

## AC. Codex's safety kernels — two reference designs

### Guardian: the isolated reviewer pattern

Codex runs a **second reviewer model with its own curated context**: eleven dedicated `guardian_*` fragments (`guardian_policy`, `guardian_review_evidence`, `guardian_conversation_history`, `guardian_tool_descriptions`, `guardian_retained_instructions`, `guardian_approved_action`, `guardian_budget_omission`, `guardian_followup_review_reminder`, `guardian_context_mode`, `guardian_sender_messages`, `guardian_node_repl_policy`) — the reviewer sees exactly what the policy decides it should, isolated from the main loop's context.

This is the second reference design for auto-approval (item 20), beside fabric's inline Jev classifier: **isolated-context reviewer** (more tokens, higher fidelity, sees conversation history and evidence) vs **typed inline classifier questions** (cheap, schema-bounded, hard escalation). Prime-agent can choose per cost profile — or run the classifier inline and escalate to a reviewer on hard judgments, which is exactly the fabric escalation ladder.

### execpolicy: the command-policy DSL

`codex-rs/execpolicy/` is a full policy engine: a parser with positioned errors (`TextPosition`/`TextRange`), rules and decisions, amend operations (`blocking_append_allow_prefix_rule`, `blocking_append_network_rule`), and sandbox migration. This is the Rust reference implementation for **command allowlists** — the policy layer prime-agent's unsandboxed bash lacks, and the concrete starting point for the sandboxing strategic item (pass 1 §3.9) short of full OS sandboxes.

## AD. Fabric's Python tree — the prime-agent lane

`pi-fabric/skillsets/python/` ships the same **fifteen canonical skills plus references** in Python. Since prime-agent's kernel *is* Python, this tree is the direct compatibility lane for any fabric-on-prime-agent hosting: shorter than the TS tree, same public skill names, and it would exercise prime-agent's kernel the way fabric exercises QuickJS — as the orchestration guest. This materially lowers the cost estimate of the extension-seam decision (item 18): hosting fabric's Python tree needs far less surface than the full TS extension API.

---

## AE. Ranked additions (58–60)

58. **Per-file mutation queue** — realpath-keyed serialization for kernel edit/write under concurrency (~60 lines; kernel)
59. **Guardian-pattern isolated reviewer** — the alternative auto-approval design; classifier-inline with reviewer-escalation composes both (pa-daemon, pairs with items 20/45)
60. **execpolicy command-policy DSL** — allowlist engine grounding the sandboxing strategy (pa-core tools)

Pass 12 also **closes the tool-layer comparison** opened by the audit: truncation/ANSI/output-file parity was verified there; the mutation queue was the one real gap. The borrow folder now covers every layer of the stack — wire, prompt, tools, kernel, agents, verification, and policy.
