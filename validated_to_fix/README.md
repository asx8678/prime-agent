# Validated findings to fix

Validated 2026-10-11 against HEAD `46e645655` (plus working-tree edits), using the reports in `findings/`.
Each finding was checked again against the code at HEAD. "Reproduced" means the behavior was triggered for real in this pass. "Code-confirmed" means the cited code does what the finding says.

**Parity caveat:** the TS ground-truth checkout (`~/prime-agent`) does not exist on this machine, so I could not check any claim about TS parity. Every "parity" note below is an open check for whoever implements the fix (AGENTS.md merge gate).

Nothing here is fixed yet. Tier 1 items are ready to hand off. Tier 2 items need a maintainer decision first.

---

## Tier 1: validated defects to fix

### 1. [High] Path traversal in `git:` package sources reaches `remove_dir_all` outside the install root
*Source: findings/app-audit/01 · Status: **reproduced***

**Locations**
- `crates/pa-core/src/packages/source.rs:236-262`: `parse_generic_git_url` only checks the segment count (`:250`) and accepts `..` and absolute segments. The hosted branch (`:178-215`) builds `path` from an unvalidated `project` the same way. `host` is not validated either (`https://../../x/y` gives `host = ".."`).
- `crates/pa-core/src/packages/git.rs:14-31`: `git_install_path` joins `host`/`path` raw for all three scopes.
- `crates/pa-core/src/packages/git.rs:52-66` (`install_git`: `create_dir_all(parent)` + clone), `:197-203` (`update_git`: `reset --hard` + `clean -fdx`), `:304-323` (`remove_git`: `remove_dir_all` with no containment check).
- Variants: `packages/npm.rs:118-133` (`npm_install_path` joins `source.name` raw, read-only escape) and `packages/mod.rs:131-146` (`temporary_dir` suffix).
- The attack can come from repo-controlled config: `<cwd>/.prime/agent/settings.json` packages are honored (`manager.rs:382-390`).

**Repro (contained, verified this pass)**
```sh
S=$(mktemp -d); mkdir -p $S/victim_dir; echo keep > $S/victim_dir/keep.txt
export PRIME_AGENT_CODING_AGENT_DIR=$S/agent
prime-agent package install "git:https://evil.invalid.test/a/../../../../victim_dir"   # clone fails, but creates $S/agent/git/evil.invalid.test/a
prime-agent package remove  "git:https://evil.invalid.test/a/../../../../victim_dir"   # prints "No matching package found"...
ls $S    # ...yet victim_dir is gone; only "agent" is left
```

**How to fix**
1. Add one validator in `packages/source.rs`, for example `validate_relative_component(s)`. It must reject empty, `.`, `..`, absolute or rooted components, and backslashes in `host` and in each `path` segment. Apply it in both git branches (generic and hosted) and to npm `name` (scoped `@scope/name` is still allowed). Watch out: `parse_source` (`source.rs:356-377`) has no error variant. A `None` from `parse_git_url` falls through to `ParsedSource::Local`, so just returning `None` would turn the crafted string into a "local path". The rejection must surface as an error: either add an invalid/`Result` outcome to `parse_source`, or validate in the manager entry points (`manager.rs:238-297`) before dispatching.
2. Defense in depth: in `install_git`, `update_git` and `remove_git`, resolve the target lexically and `bail!` unless `target.starts_with(root)`. Do this **before** `create_dir_all`, clone, `reset`/`clean`, or `remove_dir_all`. `git_install_root` (`git.rs:34-40`) returns `None` for `Temporary`. For that scope, check against the `temporary_dir("git-<host>", None)` base instead, or make the root function total.

**Regression tests** (each fails without the fix)
- `source.rs` tests: `git:https://evil.test/a/../../x`, `git:https://../../x/y`, `https://github.com/u/..`, and `git:git@host:/abs/path` are rejected. Compare whole objects (`parse_source(..) == Err/None`).
- `packages/tests.rs`: create a sibling `victim/` dir outside the sandbox install root, call `remove_git` with an escaping `GitSource` built directly (which skips the parser), and assert `Err` **and** that `victim/keep.txt` still exists.

**Expected outcome**
- `prime-agent package install "git:https://x.test/a/../../v"` is rejected with an error before any filesystem access. No directories are created, and nothing is treated as a local path.
- `remove`/`update` can never touch a path outside `<install_root>`. The repro above leaves `victim_dir/keep.txt` intact.

**Parity:** unverified whether TS validates sources. If TS lacks it, note the divergence in the PR. Do not copy the vulnerability.

---

### 2. [Medium] Package tests need a system `npm` and poison a shared mutex (1 real failure shows up as 6)
*Source: findings/app-audit/03 · Status: **reproduced** (`cargo test -p pa-core --lib -- packages::` → 73 passed, 6 failed; 1× `spawn npm ENOENT` at `tests.rs:254`, 5× `PoisonError`)*

**Locations**
- `crates/pa-core/src/packages/tests.rs:152,182,199,221,236,297`: `ENV_MUTEX.lock().unwrap()`.
- `crates/pa-core/src/packages/tests.rs:247-253` and `:315+` (plus the `update_git` calls at `:270,:280`): pass `npm_command: None`, which skips the sandbox npm shim the npm tests use.
- `crates/pa-core/src/packages/mod.rs:149-153`: the shared `ENV_MUTEX`.

**How to fix**
- Replace the 6 guards with `.lock().unwrap_or_else(std::sync::PoisonError::into_inner)`. Check the other users of `test_support::ENV_MUTEX` (`packages::resolve::tests`, `session_engine::telemetry::tests`) and change them too.
- In both git tests, pass the sandbox's npm shim (the one written at `tests.rs:122-131`) to `install_git`/`update_git` instead of `None`, as the npm tests do.

**Regression check:** run the suite with `npm` off `PATH` (`env PATH=<dirs without npm> cargo test -p pa-core --lib -- packages::`). It fails today and must pass after the fix.

**Expected outcome:** `cargo test -p pa-core --lib` passes on a machine without npm. If one test panics in the future, it reports only its own failure, not 5 extra `PoisonError`s.

---

### 3. [Medium] Untriaged informational advisories (`ring` unmaintained, 2× `lru` unsound)
*Source: findings/app-audit/04 + 05 · Status: **code-confirmed** (`deny.toml` ignore list holds only `RUSTSEC-2023-0071` and `RUSTSEC-2024-0436`; `Cargo.lock` has `lru 0.12.5` via `ratatui 0.29`)*

**How to fix (this item)**
- First run `cargo deny check advisories` to confirm the three IDs resolve. An `ignore` entry for an ID that doesn't exist will itself warn.
- Add dated `ignore` entries to `deny.toml`, each with a reason and a review date, in the same style as the existing ones:
  - `RUSTSEC-2025-0007` (ring unmaintained). 0.17.14 is the latest release, so there is nothing to bump to. It comes in through rustls. Re-review when rustls changes its default provider or ring publishes a release.
  - `RUSTSEC-2026-0002` and `RUSTSEC-2026-0253` (lru unsound). The only user is ratatui 0.29 `layout.rs` (`LruCache<(Rect, Layout), …>`). It calls neither `iter_mut` nor `pop`, and the key has no panicking `Drop`. Re-review when ratatui is bumped.
- **Separate follow-up PR (not this item):** ratatui 0.30.2 drops `lru` but requires crossterm 0.29, which means re-porting `vendor/crossterm` (see `PRIME_AGENT_PATCH.md`). Also put the patch-level bumps (tokio, hyper, h2, serde_json, quinn-proto) in their own maintenance PR with `Cargo.lock` regenerated.

**Expected outcome:** `make deny` stays green, and every known advisory affecting the lockfile has a recorded, dated decision.

---

### 4. [Low] A signal-killed `!` command is summarized as "failed to start"
*Source: findings/app-audit/06 · Status: **code-confirmed***

**Locations**
- `crates/pa-core/src/session_engine/structural_compaction/events.rs:196-199`: `(cancelled: false, exit_code: None)` → `BashOutcome::Failed`.
- `crates/pa-core/src/session_engine/structural_compaction/state.rs:274`: `Failed` → `"failed to start"`.
- `crates/pa-daemon/src/user_bash.rs:553-561`: `exit_code = status.code()`, which is `None` for a signal death (OOM-kill, SIGKILL, SIGSEGV). `cancelled` is only the abort flag.

**How to fix (recommended: option a)**
- **(a) Minimal, no wire change:** rename the outcome and label so they are truthful for both cases. For example, `BashOutcome::NoExitCode` → `"ended without an exit code"` (killed by a signal or failed to start). This needs no row change and carries no parity risk.
- (b) Richer: persist a `failedToStart` marker or `errorMessage` in the BashExecution row at `user_bash.rs:594-616`. This changes the durable session JSONL shape, which is pinned to TS, so it needs the parity check first. Track it as a Tier 2 decision if you want it.

**Regression test:** update `structural_compaction/tests.rs:281-313`. The `(false, None)` row must render the new neutral label, not "failed to start", using a whole-string comparison of the rendered summary line.

**Expected outcome:** after compaction, an OOM-killed `!build` reads `- bash: build: ended without an exit code`. It no longer makes a false claim about a run the user watched.

---

### 5. [Low] SSE decoder splits a CRLF that straddles a chunk boundary into two line breaks
*Source: findings/app-audit/07 · Status: **code-confirmed***

**Locations**
- `crates/pa-ai/src/utils_inner/sse.rs:80-95`: a `\r` at the end of the buffer matches `b'\r' | b'\n' => 1`, so the next chunk's leading `\n` becomes an empty line and flushes the event early.
- This decoder is shared by the anthropic, openai_completions, google, google_vertex, mistral and azure_openai_responses providers.

**How to fix:** carry a `pending_cr` flag (preferred). If a chunk ends in `\r`, emit that line break and set the flag. If the next chunk starts with `\n` while the flag is set, skip that `\n`. Don't simply keep the `\r` in the buffer: `push_text` starts scanning at the pre-append `buffer.len()`, so a held `\r` would be skipped and the line would carry a stray CR. That approach only works if the scan start is rewound by one. Let `flush`/end-of-stream treat a trailing lone `\r` as a line break.

**Regression test:** add `crlf_split_across_chunks_is_one_line_break`. Push `"data: a\r"`, `"\ndata: b\r"`, `"\n\r\n"` and assert **one** event with `data == "a\nb"`. Today this yields 2 events. The existing `preserves_fragmented_line_breaks_and_raw_lines` (`sse.rs:178`) pins the bug. It expects `"data: first\r"` + `"\n"` to emit the event, which only happens because the split CRLF counts as line + empty line. Update it to the spec-correct expectation: no event until a real blank line arrives.

**Expected outcome:** the same byte stream produces identical events no matter where the chunk boundaries fall.

**Parity:** the file header says it "Mirrors the TS SSE decoder". The TS SDK line decoders hold back a trailing `\r`. Confirm this against the TS binary/SDK before merging.

---

### 6. [Low] Auto-compaction never triggers on small-window models (threshold saturates to 0)
*Source: findings/tokens-and-context/10 · Status: **code-confirmed**, and models in the shipped catalog are affected*

**Locations**
- `crates/pa-core/src/session_engine/compaction.rs:154-166`: `threshold = min(0.95·W, W − max_output − max(reserve, 4096))`. When the second term saturates to 0, `should_compact` is permanently false.
- `crates/pa-ai/src/models_generated.rs` has entries with `context_window = 8192, max_tokens = 8192`: 8192 − 8192 − 16384 saturates to 0. The caller passes the *effective* request budget (`min(max, 32000)`), so large-window rows with `max_tokens == context_window` (e.g. 131072) are **not** affected. Only the small-window entries are.

**How to fix:** the output reservation is already a deliberate Rust-only divergence (see the doc comment at `:150-152`), so fixing it has no parity concern. Cap **both** reservations relative to the window (capping only the reserve does nothing when `max_output == W`): `combined = W − min(max_output, W/2) − min(headroom, W/4)`. For 8192/8192 that gives 8192 − 4096 − 2048 = 2048 > 0. Update the doc comment.

**Regression test:** `compaction_threshold(8192, 8192, &default_settings) > 0`, and `should_compact` is true at ~90% of an 8k window.

**Expected outcome:** small-window sessions compact through the trigger path instead of hitting a context overflow first.

---

### 7. [Low, latent] The compaction trigger's estimate ignores Image blocks in plain user messages
*Source: findings/tokens-and-context/16 · Status: **code-confirmed***

**Locations:** `crates/pa-core/src/session_engine/compaction.rs:77-91`. The `User` arm's `filter_map` keeps only `Text` (with a `_ => None` wildcard). The `Custom` and `ToolResult` arms count `Image(_) => 4_800` chars.

**How to fix:** replace the wildcard with an exhaustive match. Count `UserContentBlock::Image(_) => 4_800`, the same as the `Custom` arm. This also removes a wildcard arm that AGENTS.md forbids.

**Regression test:** `estimate_tokens` of a `User` message with one Image block equals `1_200`.

**Expected outcome:** the token estimate counts every image, whatever message kind carries it. This is latent today because `attach_image` produces Custom rows.

**Parity:** check whether TS `estimateTokens` counts user images. If it doesn't, this is still a correctness fix to the trigger (it changes no wire output).

---

## Tier 2: validated, but needs a product/parity decision before fixing

| # | Finding | Validated | Decision needed | Recommendation |
|---|---|---|---|---|
| A02 | Provider/proxy bodies have no idle timeout. `pa-ai/src/utils_inner/http.rs:20-38` (only `pool_idle_timeout`), `:252-258` (no deadline when an abort signal is present), `pa-agent/src/proxy.rs:265` (`Client::new()`) | code-confirmed | Is this a deliberate TS-parity choice? If the TS SDK clients apply a default timeout (Anthropic/OpenAI SDKs do have request timeouts), this is a parity **gap** and moves to Tier 1. | Add an **idle** deadline on body reads (no bytes for N s), not a wall clock, so long thinking pauses still work. It should surface `ProviderConnectionError { kind: Timeout }` so the existing retry/failover path handles it. Test: a mock server sends 200 plus headers and then stalls, and the turn fails with Timeout within N s. |
| T01 | The whole system prompt is one cached block, and the static prefix (`cached_prefix_len`, 23,883 ch) has no breakpoint of its own. `pa-ai/src/providers/anthropic/params.rs:55-77` | code-confirmed | Wire shape differs from TS: change it? | Yes. Split it into [static layers + cache_control][dynamic tail]. That uses 4 breakpoints, which is the limit. |
| T02 | 5-minute cache TTL by default; 1h only via the `PI_CACHE_RETENTION=long` env var. `anthropic/mod.rs:173-181`, `bedrock/mod.rs:301-308` | code-confirmed | Keep the env var for parity, and add a `cacheRetention` setting? Change the default? | Add a settings key and default interactive sessions to `long` when `supports_long_cache_retention`. Fix both providers. |
| T04 | `[harness-digest]` boilerplate (~2.6k ch) on every request, even when all counts are 0. It is also reported as duplicated after compaction | boilerplate code-confirmed (`harness_digest.rs:136-160`). The duplicate-after-compaction claim (`append_stale_harness_digest`, `:445+`) is **not independently validated**: it rests on mock captures | Is it OK to collapse the digest when all counts are 0? | Reproduce the duplicate first. Then use a one-line digest when empty, and exactly one digest row after compaction. |
| T05 | Compaction summaries and the auto-refine gate run on the session model. `auxiliary_model.rs:53-95` | code-confirmed | Default `auxiliaryModel` to a cheap model? | Only after the LLM-engine quality has been tested on a real model (see findings 14). |

## Reviewed and excluded (not defects to fix now)

- **T03** (deterministic compaction default): documented as intentional (`567ab02db`). Findings 14 and 15 also showed its LLM-engine evidence was canned mock text.
- **T07** (OpenAI-compat `prompt_cache_key` only for api.openai.com) and **T11** (`max_tokens / 3` fallback, `anthropic/params.rs:38`): the code is confirmed. They are excluded as low-impact because they are believed to mirror TS behavior, but **TS parity is unverified**. If TS differs, T11 moves to Tier 1.
- **T06** (skills inventory size), **T08** (@file insert cap), **T09** (loop detection), **T17** (64 KiB subagent answer cap): tuning or feature ideas, not defects.
- **12, 13, 14, 15, 18, 19** and app-audit **08, 09, 10**: measurement, evidence and coverage notes. There is nothing to fix in them.
