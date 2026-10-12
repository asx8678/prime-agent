# The RLM API trio — decision record (M6 unblocked)

Date: 2026-10-12. The implementation plan's M6 gate blocked three `rlm.spawn`
extensions on one decision taken together with the TS side. The Rust-side
rulings are recorded here; **TS sign-off is still pending** — this record, or
the brief below, goes to the TS side before PR-10 merges. The three stay
separate decisions: if one stalls on the TS side, the other two can still
ship.

## 0 · Entry point

**Yes — keyword arguments on `rlm.spawn`, not a second entry point.** The host
already rejects keywords it does not know (`rlm_host.rs:504`), so an older
host that receives a new keyword fails loudly instead of ignoring it, and
one call keeps TS and Rust in step.

## 1 · `schema=` — validated child results (PR-10)

- **1a. Plain JSON Schema dicts.** Fabric checks a plain object with
  `Value.Check(schema, value)` (`result.ts:57`); a TypeBox schema is a JSON
  Schema object once it runs, so the two options are the same thing. Use a
  plain JSON Schema dict in Python — fabric schemas carry over unchanged.
  `jsonschema` 4.26 is already in kernel-venv. Error wording will differ
  from TypeBox's; do not promise identical error text.
- **1b. No hidden retry.** Fabric's plain-text-to-action conversion for one
  agent kind (`result.ts:63-80`) is explicitly **not** ported.
- **1c. Validate in the parent kernel, on `collect`.** Not in the child: the
  child's REPL never sees the final reply the host assembles, so there is
  nowhere inside the child to validate it. Not in Rust either — a JSON
  Schema crate on the host would be an architecture change. The schema is
  stored on the spawn handle and applied when the parent calls `collect`,
  with `jsonschema` in the kernel.

## 2 · Tree cost ceiling (PR-11)

- **2a. USD plus a spawn-call cap, and unpriced models are refused.** Models
  the catalog has no price for (`cost: (0.0, ...)`) would never trip a USD
  ceiling; when a USD ceiling is set, spawning a child on an unpriced model
  is refused.
- **2b. Scope: the whole tree under the root session.** "Per root execution"
  is ambiguous — children outlive the cell and the turn that started them;
  a per-cell or per-turn budget would reset while those children keep
  spending.
- **2c. Opt-in.** `None` = uncapped, today's behavior.
- **2d. Refuse before the spawn starts, with a catchable error.** Parallel
  spawns can overshoot: several spawns in one cell all pass the check
  together, so the overshoot is bounded only by the number of children
  already running. **No new JSONL ledger**: child usage is already summed
  for telemetry and stored in each child's session file — a counter in the
  host, rebuilt from those files on reattach, is enough.
- **2e. The parent's own spend counts** toward the ceiling.

## 3 · Write policy and worktrees (split)

- **3b fails as a tool-level policy.** Fabric blocks write/edit calls outside
  the allowed folders and refuses shell outright unless the parent passes
  `"unconfined"` (shell cannot be confined; `write-guard.ts:81`). Our
  children call `bash` and `ipython`, and `ipython` is a Python process that
  can `open()` any file — so `readOnly`/`writableRoots` could only be
  enforced by taking away `bash` and `ipython`, leaving the child almost
  useless. A policy that claims read-only while Python can still write
  anywhere is worse than no policy, because people will rely on it. **The
  write policy is on hold** until there is a real OS sandbox around the
  kernel and bash (Landlock/bwrap on Linux, Seatbelt on macOS) — a separate,
  larger decision.
- **3c worktrees: REJECTED (ruling updated 2026-10-12).** Prime Agent will
  not use worktrees. The earlier ship-opt-in-isolation ruling is withdrawn;
  nothing in this repo spawns git worktrees for children. Isolation, when it
  is ever wanted, must come from the same OS-sandbox decision as the write
  policy — not from worktree folders.

## Order

PR-10 `schema` (done) → PR-11 cost ceiling. **PR-12 worktrees is
cancelled** — see §3.

## Brief for the TS side

The one question TS must answer: do these land as `rlm.spawn` keyword
arguments with these exact names and semantics in the TS product too —
`schema=` (JSON Schema dict, validated at collect, first-5 errors, no
retry), the cost-ceiling keyword (name TBD in PR-11's design pass), and the
worktree flag (name TBD in PR-12's design pass)? Rust implements the
surface; parity requires TS to show the same parameters eventually.

## Update 2026-10-12 — tree scope implemented

Ruling 2b now holds below depth 1: at each spawn under a ceiling, the child
inherits the parent tree's remaining budget (USD and spawn admissions)
through its create's `runtimeMetadata.rlmCostCeiling`, enforced beside the
child's own ceiling (tighten only, never loosen) and passed on to its own
children. The ceilings and admission count persist in the session log. The
new metadata key is a shared-surface change pending the TS answer, tracked
with the other divergences in `next-steps/09-rust-divergence-signoff.md`.
