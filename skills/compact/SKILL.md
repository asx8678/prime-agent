---
name: compact
description: Check context usage and compact the conversation from the Python REPL. Use when context is filling up and substantial work remains, so the session is summarized and you keep working instead of stopping early.
---

# Compact

Compaction replaces older conversation history with a dense summary, freeing
context so long-running work can continue. The implementation lives in the
host (the same one behind the user's `/compact` command); this skill is the
kernel-side interface to it. Call it directly from the Python REPL:

```python
await compact.status()
await compact.run()
await compact.run("keep the failing test names and the migration checklist")
await compact.carry("the API must stay stable")
await compact.drop(0)
await compact.run(preserve=["the exact error text"])
```

## API

- `await compact.status()` — current context usage as a dict: `tokens`,
  `context_window`, and `percent` (`None` right after a compaction until the
  next model response), plus `scheduled` (whether a requested compaction is
  already pending) and `carry` (the carried notes, indexed as `drop` sees
  them).
- `await compact.run(instructions=None, preserve=None)` — schedule compaction.
  Returns `{"scheduled": True}`, or `{"scheduled": False, "reason": ...}`
  when there is nothing to compact yet. Optional `instructions` focus the
  summary on what matters for the remaining work; optional `preserve` (a
  list of strings) names items kept verbatim in this summary only.
- `await compact.carry(note)` — pin a short note that every later summary
  includes in its own `[Carried notes]` block, until dropped. Returns the
  current list (also visible via `status()`). Bounded: at most 16 notes,
  each one short line; adding an existing note changes nothing.
- `await compact.drop(index)` — remove the carried note at `index` (the
  indexes `status()` and the `[Carried notes]` block print).

## Rules

- Compaction never runs mid-cell. A scheduled compaction runs when the
  current turn ends; the harness then resumes you automatically with the
  summary plus recent messages, and you continue the task.
- The Python kernel persists through compaction — variables, imports, and
  helpers you defined all remain available.
- Compact at a natural boundary when context usage is high and substantial
  work remains, instead of becoming terse or returning to the user early.
  Check `await compact.status()` when unsure.
- One request per turn is enough; calling `run` again before the turn ends
  only updates the instructions.
- Carry durable constraints (an API that must not change, a repro the
  summary must keep naming), not transient state: a carried note renders in
  *every* summary until you `drop` it. Prefer `run(preserve=[...])` for
  one-shot must-keep items of the very next summary.
