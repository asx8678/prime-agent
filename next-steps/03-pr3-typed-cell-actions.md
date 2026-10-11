# 3. PR 3: typed per-cell actions

Size: about 400 lines of code and 400 of tests, 2-4 days. Medium risk.

## Why

Today the deterministic summary knows, per Python cell: its status, the
exception (`Name: message`), the first `bash()` command and how many ran,
the files `edit()` changed, and the agent messages it sent (`CellOutcome`
in `structural_compaction/events.rs`).

It does not know:
- which subagents a cell spawned;
- goal and heartbeat calls;
- MCP calls and their errors;
- harness writes;
- the exit code of an awaited `bash()`.

That last gap matters most. An awaited `bash()` that exits non-zero still
leaves the cell "ok", so a failed build or test run is invisible to the
summary's failure list.

## Where the facts are

- **Host requests**: `rlm.spawn`, goal and heartbeat calls, `mcp.*` and
  harness writes reach the host as host requests. Record a typed action per
  cell at `start_host_request`
  (`crates/pa-core/src/kernel/manager/host_requests.rs:27`).
- **Awaited `bash()` exit codes** are only known in the Python runtime
  (`prime-agent-runtime/src/rlm/bash.py`). Emit one kernel display event
  when an awaited command finishes. `mcp.py` and `harness.py` in the same
  folder can report through the same event if the host request does not
  carry enough.
- **Carry them in the tool-result details** as
  `actions: [{kind, subject, outcome}]`. There are precedents in
  `crates/pa-core/src/tools/ipython.rs`: `bashCommands` (line ~459) and
  `diffs` (PR 0). Thread the field through
  `session_engine/runtime_wiring.rs` (`convert_execute_result`), where PR 0's
  `diffs` had been dropped.

## Use them in the summary

- `events.rs`: `CellOutcome` gains the actions.
- `state.rs`: a failed awaited command opens a `FailureKind::Command`
  failure, and the same command later exiting 0 resolves it. Spawns go to
  the activity sample.
- `render.rs`: list them in "Agents And Activity" and "Outstanding
  Context".
- Bump `STATE_VERSION` if the saved shape changes. A state saved by an older
  version is then carried once as an earlier-summary excerpt.

## Watch out

- The details keys are on the wire. The TUI must ignore keys it does not
  know: check `crates/pa-tui/src/tool_card/ipython_details.rs`. The TS
  product does not write these keys.
- Keep text bounded (subjects about 100 characters), and never put output
  bodies in the details.

## Tests

- A kernel test per action kind; the kernel tests live in
  `crates/pa-core/tests/kernel_*.rs`.
- Update the golden summary fixture with a cell whose awaited `bash()` exits
  non-zero, and its later fix.
