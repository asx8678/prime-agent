# Next steps: deterministic compaction and what follows

Saved 2026-10-11, committed on `main`. Update it as work lands.

## Where things stand

`main` = `origin/main` = `31857a7eb` on `asx8678/prime-agent`:

| Commit | What it did |
|---|---|
| `016489755` | PR 0: Python-cell `edit()` diffs are saved in the tool-result details |
| `7d3e1c6ea` | PR 1: the structural compactor (`crates/pa-core/src/session_engine/structural_compaction/`) |
| `5b2fcacfa` | Fix: a worker's first prompt was sent to the model twice |
| `275b44bb2` | PR 2: the `compaction.engine` setting, `structuralState` on compaction rows, daemon re-pin with the engine's keep window, telemetry |
| `567ab02db` | The deterministic engine became the default (`"llm"` still selects the model summarizer) |
| `31857a7eb` | Merge of `rust-1-99-toolchain` (made by another Claude session): Rust 1.99.0 is pinned in `rust-toolchain.toml`, so plain `cargo` uses it |

## Order of work

1. [01-verify-main.md](01-verify-main.md): parts of `main` were pushed
   without a full test run. Do this first.
2. [02-pr4-history-skill.md](02-pr4-history-skill.md): let the model expand
   `[u12]` / `[u12.c3]` addresses from the summary. Most valuable next,
   now that the deterministic engine is the default.
3. [03-pr3-typed-cell-actions.md](03-pr3-typed-cell-actions.md): richer
   per-cell facts in the summary.
4. [04-pr5-carry-and-preserve.md](04-pr5-carry-and-preserve.md): notes
   that survive every compaction.
5. [05-context-folding.md](05-context-folding.md): fold finished sub-tasks
   out of the live context (largest, riskiest; behind a setting).
6. [06-follow-ups.md](06-follow-ups.md): smaller issues found along the
   way, and housekeeping.

## How to work in this repo (lessons from this round)

- **Work on `main` only; do not create branches** (owner's rule). Work in
  this checkout, `/Users/adam2/projects/prime-agent`.
- **Another Claude session may work in this checkout too.** It merged and
  pushed `main` itself, and once wiped uncommitted changes. So commit small
  and often, stage only your own paths (never `git add -A`), and run
  `git fetch && git status` before committing to see what moved.
- **Repo rules** (`AGENTS.md`): never set the git identity, never add
  `Co-authored-by` trailers, one logical change per commit, a regression
  test that fails without each fix, adoption telemetry in the same change as
  a user-visible feature, and no "pi"/"fabric" names in anything visible.
  The repo treats any user-visible difference from the TS product as a parity
  question; record each one in the commit message.
- **Ask before pushing, merging or deleting.**
- **Gates:** `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`. On this Mac they are never fully green:
  - several `crates/pa-cli/tests/*` files are Linux-only (`prctl`,
    `termios.c_line`, `ptsname_r`) and do not compile;
  - `vendor/crossterm` has clippy findings;
  - about 60 tests fail for environment reasons (Unix socket paths over the
    macOS limit, `/proc`, `/private/var`).

  Compare failures against [known-macos-failures.txt](known-macos-failures.txt)
  instead of expecting green. That list was measured with Rust 1.98.1, so
  re-baseline it on clean `main` with 1.99.
- **Test timing:** the full `pa-daemon` suite takes 25-30 minutes. When
  iterating, run single files: `cargo test -p pa-daemon --test <name>`.
  Under load, two tests have been flaky:
  `readoption_wake_e2e::a_heartbeat_keeps_firing_across_a_supervisor_restart`
  and `kernel_startup_memo::doomed_settle_keeps_the_newer_boot_listener_state`.
- **Real-model checks:** use the zro provider only, never coralbricks.
  [tools/realcheck_zro.py](tools/realcheck_zro.py) runs the branch's
  `pa-daemon` against a throwaway agent dir with only zro configured
  (synthetic prompts, telemetry off).
- **Disk:** `target/` grows to 15-40 GB.
  [tools/sweep_target.py](tools/sweep_target.py) removes build output made by
  a compiler you no longer use. After the first 1.99 build, run
  `python3 next-steps/tools/sweep_target.py target 1.99.0 --apply` to drop
  the obsolete 1.98.1 output. It refuses until 1.99 output exists; without
  `--apply` it only reports.
