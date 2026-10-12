---
name: fold
description: Fold a finished sub-task out of your context from the Python REPL. Wrap trial-and-error work (searching the codebase, a reproduction, an implementation attempt, running and fixing tests, a review pass) in fold.begin / fold.end so later turns keep only its result, and long tasks stay small and cheap.
---

# Fold

Context folding hides the work of a finished sub-task from every later
request: the turns between `fold.begin` and `fold.end` are left out, and
only the two calls stay, the second one carrying the result you write. The
transcript and the session log keep everything; only what the model is sent
changes. Call it directly from the Python REPL:

```python
await fold.begin("find the parser bug", "Locate why nested lists fail to parse; bring back the file, function and a failing input.")
# ... several cells of searching, running, trying ...
await fold.end("""Outcome: depth() in crates/foo/src/parser.rs underflows on `[[]]`.
Files changed: none. Repro: tests/repro_nested.py (kept on disk).
Key findings: only empty inner lists trigger it. Open question: does the
writer have the same bug?""")
```

## API

- `await fold.begin(description, goal)` opens a fold. `description` is a
  3-5 word label; `goal` states the objective and what the result must bring
  back. It prints the instructions for the fold.
- `await fold.end(result)` closes the open fold. It prints how much was
  folded, followed by your result. That printed result is what later turns
  read in place of the folded work.

Both print and return `None`. A fold that cannot open or close prints why.

## When to fold

- Fold trial-and-error or exploration: searching the codebase, writing a
  reproduction or a test, an implementation attempt, a review pass, running
  and fixing tests, verification.
- Keep planning, decisions and the final review outside folds.
- One fold at a time. Give each a goal that says what to bring back.

## Rules

- Call `fold.begin` and `fold.end` in separate cells, in separate turns,
  with the work in between. A cell may call only one of them.
- The result must stand on its own, because the folded work is no longer
  visible: the outcome, files changed or created, environment changes, key
  findings, assumptions and open questions.
- Kernel variables and files survive a fold; the conversation does not.
  Leave anything the next step needs in variables or files, and say so in
  the result.
- While a fold is open, each tool result reminds you of it. Once the fold
  grows past its budget, the reminder asks you to stop and call `fold.end`
  with the progress made and the work still pending. Do that.
- New input during a fold (a user message, steering, a new objective)
  cancels it: nothing is hidden, and `fold.end` says so. Start a new fold
  for the next sub-task if it still makes sense.
