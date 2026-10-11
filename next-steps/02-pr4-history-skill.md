# 2. PR 4: the `history` skill (expand `[u12]` / `[u12.c3]`)

Size: about 400 lines of code and 300 of tests, 2-3 days. Low risk.

## Why

The deterministic summary samples older facts and labels them with
positions on the branch: `u12` is the 12th user message, `u12.c3` the third
Python cell after it. The header says older facts are "sampled, not
interpreted", but the model has no way to read the full text behind an
address. Now that the deterministic engine is the default, this closes the
loop.

## API (a bundled skill, like `skills/compact`)

Python side: a new `skills/history/`, with `SKILL.md`, `pyproject.toml` and
`src/history/__init__.py`. Copy the shape of
`skills/compact/src/compact/__init__.py`: typed wrappers over
`rlm.host_request`.

- `await history.show("u12")`: user message 12 verbatim, the assistant text
  that followed, and one line per cell after it (`u12.c1`...) with its
  first statement and status.
- `await history.show("u12.c3")`: that cell's code, its output (bounded),
  its error and its edits.
- Bound every result in bytes. Return a clear error for an unknown or
  malformed address.

Host side: register `history.show` next to the compact handlers.
`TurnBoundaryRequests::register_compact_handlers`
(`crates/pa-core/src/session_engine/turn_boundary.rs:213`) is the pattern:
it reaches the session through `requests.bound()`, available while a turn
runs, which is the only time a cell can call it.

## Reading the history

- A resumed (windowed) session retains only the rows after the last
  compaction's kept boundary, and the addresses point before it. Read the
  whole log: capture `SessionManager::history_snapshot()`
  (`crates/pa-core/src/session/manager/queries.rs:30`) under the session
  lock, release the lock, then await it.
- Walk the active branch (the parent chain from the leaf), not file order:
  abandoned branches stay in the file.
- **Count addresses with exactly the fold's rules**, or `u12` will name
  different messages in the summary and in `history.show`. The rules live
  in `structural_compaction/events.rs` (`normalize`) and `state.rs`
  (`address`):
  - only real user messages count; autonomous continuation prompts
    (`AUTONOMOUS_CONTINUATION_LABEL`) are not users;
  - cells are `ipython` tool calls; the cell count resets at each user
    message.

  Factor one shared address walker into `structural_compaction` and use it
  in both `normalize` and the handler, so the two cannot drift. The
  structural compaction module is private to `session_engine`; keep the
  walker `pub(crate)`.
- Optional: store a short content hash with each address in the state (bump
  `STATE_VERSION` in `state.rs`) so the handler can warn when the log no
  longer matches what the summary saw.

## Tests

- Every address in the golden summary
  (`crates/pa-core/tests/fixtures/structural_compaction/session.summary.txt`)
  resolves to the entry it names in `session.jsonl`.
- After two compactions and a resumed window, old addresses still resolve.
- Autonomous continuation rows and branch moves do not shift the numbering.
- A kernel test through the real host bridge; see
  `crates/pa-core/tests/kernel_*.rs` for the harness.

## Gates

- A new REPL function changes the surface the model sees (see "Surface
  contract" in `AGENTS.md`). Ship it as a bundled skill and record the
  divergence from TS in the commit message.
- Telemetry: count `history.show` calls in a session counter
  (`SessionCounters` in `crates/pa-core/src/session_engine/telemetry.rs`;
  `compaction_deterministic_count` is a recent example). Add the property
  to `crates/pa-telemetry/src/catalog.rs` under `agent session ended`.
- Add a changelog fragment in `.changes/`.
- Consider one line in the summary header (`render.rs`, `HEADER`) telling
  the model it can call `history.show("u12")`. It is model-facing text, so
  keep it short.
