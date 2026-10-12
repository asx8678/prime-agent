# Rust 1.99.0 bump + code-reduction review

Date: 2026-10-12. Tree: uncommitted working tree (PR series 1–11 present).

## Phase 1 — Rust 1.99.0 bump: verification record

**The repo was already pinned to 1.99.0 everywhere.** `rust-toolchain.toml` (`channel = "1.99.0"`), and all 12 `dtolnay/rust-toolchain@fb523f4…` action pins in `.github/workflows/*.yml` (the pinned-SHA scheme: the action's parse step hard-codes 1.99.0, commented at each site). Active toolchain: `rustc 1.99.0 (b940084d7 2026-09-28)`. The only other pin is `vendor/crossterm`'s `rust-version = "1.63.0"` — upstream's own MSRV inside vendored third-party code, not a workspace member; left untouched. **No version edits were needed.** Edition (2021), Cargo.lock, and dependency versions unchanged per instructions.

Verification on 1.99.0:

| Gate | Result |
| --- | --- |
| `cargo fmt --all --check` | PASS (after one `cargo fmt --all` over the uncommitted PR-9/10/11 code — whitespace only) |
| `cargo build --locked --all-targets --all-features` | PASS (5m19s; only pre-existing vendored-crossterm warnings) |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | PASS (after adding `# Errors`/`# Panics` doc sections to two new PR-11 fns — the workspace enforces `missing_errors_doc`/`missing_panics_doc`; plus a doc-summary line) |
| `cargo test --locked --all-features` | 39 suites ok; **1 failure, environmental**: `pa-cli tests/packaged_layout_e2e::packaging_dry_run_produces_artifact` shells out to `objdump` (binutils), absent on this host (`FileNotFoundError: 'objdump'` in `scripts/release/assemble_artifacts.py:415` via `scripts/release/split_debug.py:88`). CI runners have binutils; the failure is toolchain-independent (a system binary). |
| Python build/tests | The runtime is a **pure-Python hatchling package** (no maturin/PyO3), so no Rust-in-Python build. **pytest is absent from both venvs** (`prime-agent-runtime/.venv`, no system pytest), so the Python suite (including the new `test_spawn_schema.py`, `test_cost_ceiling.py`) could not be run — no installs without approval. |

Files changed in Phase 1 (all behavior-neutral): `crates/pa-core/src/session_engine/rlm_cost.rs`, `rlm_usage.rs`, `engine.rs` (doc sections + fmt reflow).

## Tool availability

Present: ruff, ast-grep (sg), uv, rg, cargo tree. **Missing (noted once, continued manually): cargo-machete, cargo-udeps, vulture, deptry, pytest/pytest-cov.** Manual replacements were used: per-crate direct-dep usage scan (machete), private-name/constant reference scans (vulture), Cargo.toml + pyproject cross-check (deptry).

## Phase 2 — Findings

Sorted by score. `unverified` marks anything lacking tool or read proof. No proven dead code, no real bugs in production paths, no unused dependencies, no unread config keys (all 117 settings fields referenced) — this tree is unusually tight, so most findings are simplification backlog and hygiene.

## Index of finding files (one per ID, sorted by score)

| ID | Score | Lang | Category | Finding |
| --- | --- | --- | --- | --- |
| PY-01 | 75 | Python | Simplification | [ruff safe-fix backlog](PY-01-ruff-safe-fix-backlog.md) |
| PY-03 | 60 | Python | Tests | [weak assertions and dead locals in test files](PY-03-weak-assertions-and-dead-locals-in.md) |
| X-01 | 55 | Cross | Tests | [packaging e2e hard-requires objdump](X-01-packaging-e2e-hard-requires-objdump.md) |
| RS-01 | 50 | Rust | Simplification | [pedantic/nursery backlog (selective)](RS-01-pedantic-nursery-backlog-selective.md) |
| PY-02 | 50 | Python | Simplification | [manual style set (SIM105/SIM117)](PY-02-manual-style-set-sim105-sim117.md) |
| RS-02 | 40 | Rust | Simplification | [edition 2021 → 2024](RS-02-edition-2021-2024.md) |
| RS-03 | 35 | Rust | Simplification | [`use_self` (135 sites)](RS-03-use-self-135-sites.md) |
| RS-04 | 30 | Rust | Duplication | [image-dimension decoder exists twice (architecturally forced)](RS-04-image-dimension-decoder-exists-twice-architecturally.md) |
| RS-05 | 25 | Rust | Comments | [misleading parity comment](RS-05-misleading-parity-comment.md) |
| X-02 | 25 | Cross | Build | [repo-root session artifacts](X-02-repo-root-session-artifacts.md) |
| PY-04 | 25 | Python | Comments/Tests | [documented no-op expression](PY-04-documented-no-op-expression.md) |
| RS-06 | 20 | Rust | Correctness (informational) | [panic-on-poisoned-mutex idiom](RS-06-panic-on-poisoned-mutex-idiom.md) |
| X-03 | 20 | Cross | Build | [Python test tooling absent in every environment here](X-03-python-test-tooling-absent-in-every.md) |
| RS-07 | 15 | Rust | Dependencies | [duplicate majors (transitive; not actionable)](RS-07-duplicate-majors-transitive-not-actionable.md) |
| RS-08 | 15 | Rust | Tests (informational) | [allow-list audit clean](RS-08-allow-list-audit-clean.md) |

## Cleared with negative results (for the record)

- **Dead code, Python:** zero private functions or module constants with ≤1 reference in `src/rlm` (reference-scanned). Public names protected by rule.
- **Unread config keys:** all 117 `settings/types.rs` fields referenced somewhere in the tree.
- **Unused scripts:** every `scripts/*.py` is referenced by CI/Makefile/docs (incl. `generate-catalog-fixture.py`, named in AGENTS.md).
- **Stale TODO/FIXME:** none (only false-positive matches like `uXXXX-style`).
- **Commented-out code (ERA001):** both flagged sites are legitimate why-comments, not dead code.
- **Swallowed errors:** `let _ =` clusters all deliberate (fire-and-forget lanes, TS-parity drops, test cleanup).
- **todo!/unimplemented!():** zero in `crates/*/src`.

## Totals

| Category | Findings |
| --- | --- |
| Correctness | 0 real bugs (RS-06 informational) |
| Dead code | 0 proven dead |
| Duplication | 1 (RS-04, architecturally forced) |
| Simplification | 5 (PY-01, PY-02, RS-01, RS-02, RS-03) |
| Comments | 2 (RS-05, PY-04) |
| Tests | 3 (PY-03, X-01, RS-08) |
| Dependencies/build | 3 (RS-07, X-02, X-03) |

**Estimated lines removed if every item scoring ≥70 is applied:** ~20 (PY-01 only). The tree is already tight; the high-value work here is selective, not a cull.
