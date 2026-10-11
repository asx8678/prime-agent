# 1. Verify what is on main (do first)

Parts of `main` were pushed without a full test run. Expect about an hour.

## 1. Five pa-cli e2e failures after the default switch

They showed up in the first run after `567ab02db` (deterministic engine
became the default) and were never compared with the commit before it:

- `crates/pa-cli/tests/print_runtime_e2e.rs`
  - `print_mode_fork_imports_a_global_session_into_this_cwd`
  - `print_mode_persists_a_session_file_by_default`
- `crates/pa-cli/tests/acp_mode_e2e.rs`
  - `acp_daemon_attached_close_stop_failure_fences_the_session`
  - `acp_daemon_attached_default_session_persists_and_resumes`
  - `acp_daemon_attached_resident_eof_mid_turn_cancels_the_prompt`

None looks compaction-related, which points to the environment (socket
paths, `$HOME` session dirs).

How to check:

1. Read each failure message first:
   `cargo test -p pa-cli --test print_runtime_e2e -- --exact <name>`.
2. If it is not obviously environmental, compare with `275b44bb2`, the
   commit before the switch. A detached checkout creates no branch:
   `git worktree add --detach <dir> 275b44bb2`. Run the same tests there,
   then `git worktree remove <dir>`.
3. If the switch caused a failure, the cause is almost certainly one of
   these:
   - a test whose fake-provider script expects a model-written summary but
     whose settings do not pin `"engine": "llm"`;
   - a test that compacts with default settings and no `compaction` object.

   Pin `"engine": "llm"` in that test's settings, as `567ab02db` did for 33
   settings objects and 27 core-settings literals.

## 2. The pa-daemon suite was not run after the switch

`cargo test -p pa-daemon --no-fail-fast` takes 25-30 minutes. Compare the
failing names with [known-macos-failures.txt](known-macos-failures.txt): any
name not on that list is new. `crates/pa-daemon/tests/import_compaction_e2e.rs`
writes no settings, so it now runs the deterministic engine. That is fine;
it is the only compaction e2e not pinned to `"llm"`.

## 3. CI on GitHub

`gh` is not logged in on this machine. Run `gh auth login`, then
`gh run list --repo asx8678/prime-agent --limit 5` and
`gh run view <id> --log-failed`. Or check the Actions tab. CI is the only
check for the daemon file-order and golden e2e tests, which fail on macOS
for environment reasons.

## 4. TS-vs-Rust compaction comparison data

`crates/pa-daemon/testdata/acp/ts-compact_command.jsonl`,
`rust-compact_command.jsonl` and `compare_differential.py` compare a
compaction between the TS product and Rust. Find out whether anything runs
them in CI. If the Rust side is regenerated with the new default, its summary
changes. Pin `"engine": "llm"` in whatever drives that comparison, so it stays
a TS-parity check.

## 5. Re-baseline on Rust 1.99

The other session pinned Rust 1.99.0 (`rust-toolchain.toml`) and fixed the
1.99 clippy findings. Run the gates once on clean `main` with plain `cargo`,
then rewrite `known-macos-failures.txt` from the result.
