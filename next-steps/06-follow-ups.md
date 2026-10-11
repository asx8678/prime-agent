# 6. Follow-ups and housekeeping

Smaller issues found along the way, none fixed yet. Each is its own small
change with a regression test.

## Issues found

1. **The LLM engine can summarize abandoned branches.** `prepare_attempt`
   (`crates/pa-core/src/session_engine/compact_session.rs`) prepares over
   `retained_entries()` in file order. A resumed window keeps the rows of
   abandoned branches, so the model summarizer can read them. TS prepares
   over the active branch (`getBranch()`). The deterministic engine already
   filters to the active path; the `llm` path does not. Prepare the `llm`
   path over the active branch too, and test a branch move followed by a
   resume and a compaction.
2. **File lists can drop a compaction's files.** The read and modified file
   lists (`details_for` in `compact_session.rs`) only look for the previous
   compaction before the new cut. When the new cut falls before the
   previous compaction row (a short session since then), that row's file
   lists are not carried.
3. **A wrong-typed nested setting drops its whole object.** For example,
   `"compaction": {"reserveTokens": "abc"}` makes the whole `compaction`
   object load as unset, `enabled` and `keepRecentTokens` included.
   `from_value_lenient` in `crates/pa-core/src/settings/load.rs` re-parses
   only top-level keys. Only `compaction.engine` is lenient today
   (`engine_or_unset` in `settings/types.rs`), and it could become the
   general pattern.
4. **The daemon's boundary re-pin relies on both stores holding the same
   rows.** The worker re-cuts the durable boundary over the session file,
   not the engine's in-memory copy. They agree only when both hold the same
   rows. The first-prompt duplicate (`5b2fcacfa`) was one such
   disagreement. `crates/pa-daemon/tests/deterministic_compaction_e2e.rs`
   checks that each saved user count matches the session file; keep it
   passing.
5. **The installed prime-agent predates `compaction.engine`.**
   `~/.local/share/prime-agent` does not know the key and may drop it when
   it saves `settings.json`. It also still runs the model summarizer. A new
   install fixes both, and no setting is needed for the new default.
6. **TS tolerance is unverified.** Whether the TS product accepts
   compaction rows with `structuralState` and the `compaction.engine`
   setting is unknown; the TS checkout (`~/prime-agent`, per `AGENTS.md`) is
   not on this machine.
7. **The first-prompt echo now waits for the session build.** Measured
   13-23 ms instead of 7-10 ms with the fake provider. Measure it on a real
   session with skills and MCP servers configured.

## Housekeeping

- **Install a build of `main`** for daily use. `make package` builds release
  archives, and `install-rust.sh --archive <path>` installs a local archive
  (see the usage notes near line 136 of the script). Check both before
  relying on them.
- **The merged branch `rust-1-99-toolchain`** still exists locally. It was
  made by another session, and the owner's rule is to work on `main` only.
  Ask before deleting it.
- **Obsolete build output:** `target/` holds Rust 1.98.1 output that the
  1.99 pin made useless. After the first 1.99 build, run
  `python3 next-steps/tools/sweep_target.py target 1.99.0 --apply`.
- **CI:** run `gh auth login` once, so CI results can be read from the
  terminal.
