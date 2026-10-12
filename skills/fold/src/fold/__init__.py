"""Prime Agent fold skill: fold a finished sub-task out of the context.

``await fold.begin(description, goal)`` opens a fold and ``await
fold.end(result)`` closes it: every later request leaves out the work
between the two cells and keeps the result. The host decides from the live
conversation and answers the text to print plus, when one applies, the
marker this cell carries (the host side owns both). These functions only
work inside the Prime Agent Python kernel.
"""

from __future__ import annotations

import weakref
from typing import Any

from rlm import emit, host_request, repl

# Keep in sync with CONTEXT_FOLD_DISPLAY_MIME in crates/pa-core/src/kernel/shared.rs.
CONTEXT_FOLD_DISPLAY_MIME = "application/vnd.prime-agent.context-fold+json"

# The cells that already called fold.begin or fold.end, by their completion
# event. A cell's marker reaches the host only when the cell settles, so the
# host cannot see a second call in the same cell; the skill refuses it.
_fold_cells: weakref.WeakSet[Any] = weakref.WeakSet()


async def begin(description: str, goal: str) -> None:
    """Open a fold for one sub-task.

    ``description`` is a 3-5 word label; ``goal`` states the objective and
    what the result must bring back. Both stay visible after the fold. Prints
    the instructions for the fold, or why none was opened.
    """
    _require_str("description", description)
    _require_str("goal", goal)
    _claim_cell()
    _answer(await host_request("fold.begin", {"description": description, "goal": goal}))


async def end(result: str) -> None:
    """Close the open fold, keeping ``result`` in place of its work.

    The result must stand on its own: the outcome, files changed or created,
    environment changes, key findings, assumptions and open questions.
    Prints what was folded, or why nothing was.
    """
    _require_str("result", result)
    _claim_cell()
    _answer(await host_request("fold.end", {"result": result}))


def _require_str(name: str, value: object) -> None:
    if not isinstance(value, str):
        raise TypeError(f"{name} must be str, got {type(value).__name__}")


def _claim_cell() -> None:
    context = repl.current_cell_completion_context()
    if context is None:
        raise RuntimeError("fold.begin and fold.end only work in a cell, not in a background task")
    finished = context[0]
    if finished in _fold_cells:
        raise RuntimeError(
            "this cell already called fold.begin or fold.end: call the next one in a later cell"
        )
    _fold_cells.add(finished)


def _answer(reply: dict[str, Any]) -> None:
    marker = reply.get("marker")
    if marker is not None:
        emit({CONTEXT_FOLD_DISPLAY_MIME: marker})
    print(reply["text"])
