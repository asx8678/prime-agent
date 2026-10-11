# 4. PR 5: `compact.carry` and `compact.run(preserve=...)`

Size: about 200 lines of code and 200 of tests, 1-2 days. Low risk.

## What

- `await compact.carry(note)`: pin a short note that every later
  deterministic summary includes, in its own "[Carried notes]" block.
  Bounded count and size; the oldest note drops first. A way to remove a
  note (`compact.drop(index)` or `carry(..., replace=True)`) avoids stale
  pins.
- `await compact.run(instructions=None, preserve=[...])`: items kept verbatim
  in the next summary only.

## Where

- Python: `skills/compact/src/compact/__init__.py` (`status`/`run` today)
  and `skills/compact/SKILL.md`.
- Host: the `compact.*` handlers in
  `crates/pa-core/src/session_engine/turn_boundary.rs`
  (`register_compact_handlers`, line ~213). `compact.run` schedules the
  compaction with its instructions; `preserve` rides along the same way.
- Deterministic engine: carried notes live in the saved state
  (`structural_compaction/state.rs`; bump `STATE_VERSION`) and render as
  their own protected block (`render.rs`, next to "[Compaction Request]").
  `preserve` items render once.
- LLM engine: append `preserve` items to the custom instructions. Carried
  notes need somewhere to live between compactions. A custom session row
  works for both engines; the deterministic state can then mirror it.

## Tests

- Notes survive two compactions and a worker restart. The daemon e2e
  `crates/pa-daemon/tests/deterministic_compaction_e2e.rs` already restarts
  a worker between compactions; extend it.
- `preserve` shows in exactly one summary.
- The summary stays within its byte budget with many notes. The proptest in
  `structural_compaction/tests.rs` covers the budget; add notes to it.

## Gates

New REPL surface (see the bundled-skill note in `02-pr4-history-skill.md`),
a telemetry counter, and a changelog fragment.
