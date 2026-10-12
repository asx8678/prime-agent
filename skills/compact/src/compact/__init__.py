"""Prime Agent compact skill: context compaction control from the kernel.

Compaction runs host-side (the same implementation as /compact); these
functions are thin typed wrappers over the generic host bridge
(`rlm.host_request`). They only work inside the Prime Agent Python kernel.
"""

from __future__ import annotations

from typing import Any

from rlm import host_request


async def status() -> dict[str, Any]:
    """Read current context usage.

    Returns a dict with `tokens`, `context_window`, `percent` (None right
    after a compaction until the next model response), and `scheduled`
    (whether a requested compaction is already pending for this turn).
    """
    return await host_request("compact.status")


async def run(
    instructions: str | None = None,
    preserve: list[str] | None = None,
) -> dict[str, Any]:
    """Schedule context compaction.

    Compaction never runs mid-cell: it runs when the current turn ends and
    the harness resumes you automatically afterwards. Returns
    `{"scheduled": True}`, or `{"scheduled": False, "reason": ...}` when
    there is nothing to compact. Optional `instructions` focus the summary on
    what matters for the remaining work; optional `preserve` names items
    kept verbatim in this summary only.
    """
    if instructions is not None and not isinstance(instructions, str):
        raise TypeError(f"instructions must be str or None, got {type(instructions).__name__}")
    if preserve is not None and not (
        isinstance(preserve, list) and all(isinstance(item, str) for item in preserve)
    ):
        raise TypeError(f"preserve must be a list of str or None, got {type(preserve).__name__}")
    payload: dict[str, Any] = {}
    if instructions is not None:
        payload["instructions"] = instructions
    if preserve is not None:
        payload["preserve"] = preserve
    return await host_request("compact.run", payload)


async def carry(note: str) -> dict[str, Any]:
    """Pin a note that every later summary includes until dropped.

    The note renders in its own ``[Carried notes]`` block in each summary,
    so durable constraints (an API that must not change, a repro the summary
    must keep naming) survive compaction. Returns ``{"items": [...]}`` with
    the current list; ``status()`` shows it with the ``drop`` indexes.
    """
    if not isinstance(note, str):
        raise TypeError(f"note must be str, got {type(note).__name__}")
    return await host_request("compact.carry", {"add": [note]})


async def drop(index: int) -> dict[str, Any]:
    """Remove the carried note at ``index``.

    The indexes are the positions ``status()`` and the ``[Carried notes]``
    block print. Returns ``{"items": [...]}`` with the remaining list.
    """
    if not isinstance(index, int) or isinstance(index, bool):
        raise TypeError(f"index must be int, got {type(index).__name__}")
    return await host_request("compact.drop", {"index": index})
