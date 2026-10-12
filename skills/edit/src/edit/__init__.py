"""Exact single-occurrence string replacement for existing files."""

from __future__ import annotations

from pathlib import Path


async def run(path: str, old_str: str, new_str: str) -> str:
    """Replace a unique string in a file.

    ``old_str`` must appear exactly once in the file at ``path``; that match is
    replaced with ``new_str`` and the file is written back in place. Prefer this
    over rewriting a whole file for targeted edits.

    Args:
        path: File to edit, relative to the working directory, absolute, or
            `~`-prefixed (the leading `~`/`~user` is expanded to the home dir).
        old_str: Exact text to find. Must occur exactly once in the file.
        new_str: Replacement text.

    Returns:
        A short confirmation message.

    Raises:
        FileNotFoundError: If ``path`` does not exist.
        ValueError: If ``old_str`` is empty, absent, or matches more than once.
    """
    if not old_str:
        raise ValueError("old_str must not be empty")
    filepath = Path(path).expanduser()
    if not filepath.exists():
        raise FileNotFoundError(f"{path} not found")
    # Read without newline translation so untouched bytes round-trip exactly.
    with open(filepath, encoding="utf-8", newline="") as handle:
        raw = handle.read()
    needle = _to_lf(old_str)
    new_lf = _to_lf(new_str)
    if "\r" in raw:
        content = _to_lf(raw)
    else:
        # Fast path: no CR means the LF view is the raw text itself, so no
        # per-file CR bookkeeping runs on the (possibly huge) content.
        content = raw
    first = content.find(needle)
    if first < 0:
        raise ValueError(f"string not found in {path}")
    if content.find(needle, first + 1) >= 0:
        # str.count misses overlapping occurrences ("aa" in "aaa" reads as
        # unique); count the way find() sees them instead.
        occurrences = 1
        position = first + 1
        while (next_match := content.find(needle, position)) >= 0:
            occurrences += 1
            position = next_match + 1
        raise ValueError(
            f"found {occurrences} occurrences in {path}, need exactly 1 — "
            "widen the snippet to make it unique"
        )
    start_line = content.count("\n", 0, first) + 1
    match_end = first + len(needle)
    if content is raw:
        raw_start, raw_end = first, match_end
    else:
        raw_start, raw_end = _raw_offset(raw, first), _raw_offset(raw, match_end)
    if new_lf != needle:
        dominant = _dominant_newline(raw)
        replacement = new_lf.replace("\n", dominant)
        if dominant == "\n":
            # A prefix ending in a lone CR fuses with a bare LF into one
            # CRLF — one break lost; a lone CR keeps both breaks.
            if raw_start > 0 and raw[raw_start - 1] == "\r" and replacement.startswith("\n"):
                replacement = "\r" + replacement[1:]
        elif dominant == "\r":
            # A trailing lone CR fuses with a suffix starting LF the same
            # way; a bare LF keeps both breaks.
            if raw_end < len(raw) and raw[raw_end] == "\n" and replacement.endswith("\r"):
                replacement = replacement[:-1] + "\n"
        # Encode before opening: a lone surrogate in new_str must fail
        # here, while the file is still untouched (open("w") would
        # truncate first). Writing bytes keeps the write in-place (mode,
        # symlinks, hard links) and free of newline translation.
        data = (raw[:raw_start] + replacement + raw[raw_end:]).encode("utf-8")
        with open(filepath, "wb") as handle:
            handle.write(data)
    resolved_path = str(filepath.resolve())
    _emit_diff(resolved_path, needle, new_lf, start_line)
    return f"Edited {resolved_path}"


def _to_lf(text: str) -> str:
    return text.replace("\r\n", "\n").replace("\r", "\n")


def _raw_offset(raw: str, lf_index: int) -> int:
    """Raw offset of the LF-view offset ``lf_index`` (a character boundary).

    Only the two offsets a match needs are ever computed, by counting
    CRLF pairs before the target instead of building a per-file offset
    list (the list is what made large files slow and memory-hungry).
    """
    offset = lf_index
    while True:
        pairs = raw.count("\r\n", 0, offset + 1)
        if offset - pairs == lf_index:
            break
        offset = lf_index + pairs
    # The fixed point can land on the LF half of a CRLF; the character
    # begins at its CR.
    if raw[offset - 1 : offset + 1] == "\r\n":
        offset -= 1
    return offset


def _dominant_newline(raw: str) -> str:
    crlf = raw.count("\r\n")
    cr = raw.count("\r") - crlf
    lf = raw.count("\n") - crlf
    if crlf > lf and crlf >= cr:
        return "\r\n"
    if cr > lf and cr > crlf:
        return "\r"
    return "\n"


# Keep in sync with DIFF_DISPLAY_MIME in src/core/kernel/index.ts.
_DIFF_DISPLAY_MIME = "application/vnd.prime-agent.diff+json"


def _emit_diff(path: str, old_str: str, new_str: str, start_line: int) -> None:
    """Stream a diff to the host as a display event; best-effort outside the kernel."""
    try:
        from rlm import emit

        emit(
            {
                _DIFF_DISPLAY_MIME: {
                    "path": path,
                    "old_str": old_str,
                    "new_str": new_str,
                    "start_line": start_line,
                },
                "text/plain": f"Edited {path}",
            }
        )
    except Exception:
        pass
