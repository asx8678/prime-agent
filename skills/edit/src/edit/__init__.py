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
    content, raw_offsets = _lf_view(raw)
    needle = _to_lf(old_str)
    count = content.count(needle)
    if count == 0:
        raise ValueError(f"string not found in {path}")
    if count > 1:
        raise ValueError(
            f"found {count} occurrences in {path}, need exactly 1 — "
            "widen the snippet to make it unique"
        )
    match_index = content.index(needle)
    start_line = content.count("\n", 0, match_index) + 1
    raw_start = raw_offsets[match_index]
    raw_end = raw_offsets[match_index + len(needle)]
    replacement = _to_lf(new_str).replace("\n", _dominant_newline(raw))
    if raw[raw_start:raw_end] != replacement:
        # In-place write keeps the file's mode, symlinks and hard links.
        with open(filepath, "w", encoding="utf-8", newline="") as handle:
            handle.write(raw[:raw_start] + replacement + raw[raw_end:])
    resolved_path = str(filepath.resolve())
    _emit_diff(resolved_path, old_str, new_str, start_line)
    return f"Edited {resolved_path}"


def _to_lf(text: str) -> str:
    return text.replace("\r\n", "\n").replace("\r", "\n")


def _lf_view(raw: str) -> tuple[str, list[int]]:
    """Return ``raw`` with CRLF/CR as LF, plus each LF-view offset's raw offset.

    The offset list has one extra trailing entry so a match end maps too.
    """
    chars: list[str] = []
    offsets: list[int] = []
    index = 0
    while index < len(raw):
        offsets.append(index)
        char = raw[index]
        if char == "\r":
            chars.append("\n")
            index += 2 if raw.startswith("\r\n", index) else 1
        else:
            chars.append(char)
            index += 1
    offsets.append(len(raw))
    return "".join(chars), offsets


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
