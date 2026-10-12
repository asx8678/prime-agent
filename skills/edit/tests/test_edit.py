"""The edit skill against real files: byte-exact line endings and errors.

Run from the skill directory with ``src`` on ``PYTHONPATH``; the path is
also inserted here so a bare ``unittest discover`` works.
"""

import asyncio
import os
import random
import sys
import tempfile
import time
import types
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from edit import run  # noqa: E402


class EditTests(unittest.TestCase):
    def setUp(self):
        self._dir = tempfile.TemporaryDirectory()
        self.path = Path(self._dir.name) / "file.txt"

    def tearDown(self):
        self._dir.cleanup()

    def edit(self, data: bytes, old_str: str, new_str: str) -> bytes:
        self.path.write_bytes(data)
        asyncio.run(run(str(self.path), old_str, new_str))
        return self.path.read_bytes()

    def test_lf_file(self):
        self.assertEqual(self.edit(b"a\nb\nc\n", "b", "x\ny"), b"a\nx\ny\nc\n")

    def test_crlf_file_keeps_crlf(self):
        self.assertEqual(
            self.edit(b"one\r\ntwo\r\nthree\r\n", "two", "2\nTWO"),
            b"one\r\n2\r\nTWO\r\nthree\r\n",
        )

    def test_crlf_old_str_matches_crlf_file(self):
        self.assertEqual(
            self.edit(b"one\r\ntwo\r\nthree\r\n", "one\r\ntwo", "x"),
            b"x\r\nthree\r\n",
        )

    def test_lf_old_str_spans_crlf(self):
        self.assertEqual(
            self.edit(b"one\r\ntwo\r\nthree\r\n", "one\ntwo\n", ""),
            b"three\r\n",
        )

    def test_mixed_endings_untouched_outside_match(self):
        self.assertEqual(
            self.edit(b"a\r\nb\nc\r\nd\re\r\n", "b", "B"),
            b"a\r\nB\nc\r\nd\re\r\n",
        )

    def test_lone_cr_preserved(self):
        self.assertEqual(self.edit(b"a\rb\rc", "b", "x\ny"), b"a\rx\ry\rc")

    def test_bom_preserved(self):
        self.assertEqual(
            self.edit(b"\xef\xbb\xbfhi\r\nthere\r\n", "there", "you"),
            b"\xef\xbb\xbfhi\r\nyou\r\n",
        )

    def test_noop_edit_does_not_rewrite(self):
        data = b"a\r\nb\nc\r"
        self.path.write_bytes(data)
        os.utime(self.path, ns=(1_000_000_000, 1_000_000_000))
        asyncio.run(run(str(self.path), "b", "b"))
        self.assertEqual(self.path.read_bytes(), data)
        self.assertEqual(self.path.stat().st_mtime_ns, 1_000_000_000)

    def test_not_found(self):
        self.path.write_bytes(b"a\n")
        with self.assertRaisesRegex(ValueError, "string not found"):
            asyncio.run(run(str(self.path), "zzz", "y"))
        self.assertEqual(self.path.read_bytes(), b"a\n")

    def test_multiple_matches(self):
        self.path.write_bytes(b"a\r\na\r\n")
        with self.assertRaisesRegex(ValueError, "found 2 occurrences"):
            asyncio.run(run(str(self.path), "a\r\n", "y"))
        self.assertEqual(self.path.read_bytes(), b"a\r\na\r\n")

    def test_empty_old_str(self):
        self.path.write_bytes(b"a\n")
        with self.assertRaisesRegex(ValueError, "old_str must not be empty"):
            asyncio.run(run(str(self.path), "", "y"))
        self.assertEqual(self.path.read_bytes(), b"a\n")

    def test_non_utf8_fails_closed(self):
        self.path.write_bytes(b"\xff\xfea\n")
        with self.assertRaises(UnicodeDecodeError):
            asyncio.run(run(str(self.path), "a", "b"))
        self.assertEqual(self.path.read_bytes(), b"\xff\xfea\n")

    def test_large_lf_file_edits_quickly(self):
        # The LF view must not run a per-character Python loop over the
        # file: a ~100 MB file would cost ~1 GB in the kernel process.
        line = "x" * 63 + "\n"
        body = (line * 312_500)[:20_000_000]
        data = body.encode() + b"UNIQUEMARK\n" + body[:1000].encode()
        self.path.write_bytes(data)
        start = time.perf_counter()
        asyncio.run(run(str(self.path), "UNIQUEMARK", "UNIQUEMARK2"))
        elapsed = time.perf_counter() - start
        self.assertEqual(
            self.path.read_bytes(),
            body.encode() + b"UNIQUEMARK2\n" + body[:1000].encode(),
        )
        self.assertLess(
            elapsed,
            1.0,
            f"editing a 20 MB LF file took {elapsed:.2f}s; the LF view "
            "must not walk the file per character",
        )

    def test_identity_edit_spans_non_dominant_ending(self):
        # old_str == new_str spanning an LF ending in a CRLF-dominant file
        # must not rewrite the matched span's line endings.
        data = b"a\r\nb\nc\r\nd\r\n"
        self.path.write_bytes(data)
        os.utime(self.path, ns=(1_000_000_000, 1_000_000_000))
        asyncio.run(run(str(self.path), "b\nc", "b\nc"))
        self.assertEqual(self.path.read_bytes(), data)
        self.assertEqual(self.path.stat().st_mtime_ns, 1_000_000_000)

    def test_replacement_after_lone_cr_keeps_both_breaks(self):
        # A prefix ending in a lone CR joined to a replacement starting
        # with a break must not fuse into one CRLF (one break lost).
        self.assertEqual(
            self.edit(b"a\rb\nc\nd\n", "b", "\nB"),
            b"a\r\rB\nc\nd\n",
        )

    def test_replacement_before_lf_keeps_both_breaks(self):
        # A replacement ending in a lone CR (the dominant ending) joined to
        # a suffix starting with LF must not fuse into one CRLF.
        self.assertEqual(
            self.edit(b"a\rb\nc\rd\re", "b", "B\n"),
            b"a\rB\n\nc\rd\re",
        )

    def test_overlapping_matches_are_multiple(self):
        self.path.write_bytes(b"aaa")
        with self.assertRaisesRegex(ValueError, "found 2 occurrences"):
            asyncio.run(run(str(self.path), "aa", "b"))
        self.assertEqual(self.path.read_bytes(), b"aaa")

    def test_overlapping_matches_across_crlfs_are_multiple(self):
        # str.count sees only one "\n\n" here; the second, overlapping
        # occurrence must still be reported.
        self.path.write_bytes(b"\r\n\r\n\r\n")
        with self.assertRaisesRegex(ValueError, "found 2 occurrences"):
            asyncio.run(run(str(self.path), "\n\n", "b"))
        self.assertEqual(self.path.read_bytes(), b"\r\n\r\n\r\n")

    def test_diff_payload_is_lf_normalized(self):
        captured: list = []
        fake_rlm = types.ModuleType("rlm")
        fake_rlm.emit = captured.append
        self.path.write_bytes(b"one\r\ntwo\r\n")
        sys.modules["rlm"] = fake_rlm
        try:
            asyncio.run(run(str(self.path), "one\r\ntwo", "x"))
        finally:
            del sys.modules["rlm"]
        self.assertEqual(
            captured[0]["application/vnd.prime-agent.diff+json"],
            {
                "path": str(self.path.resolve()),
                "old_str": "one\ntwo",
                "new_str": "x",
                "start_line": 1,
            },
        )

    def test_encoding_error_leaves_file_untouched(self):
        # A lone surrogate in new_str must fail before anything is
        # written; the old open("w") truncated the file to zero bytes.
        data = b"a\nb\n"
        self.path.write_bytes(data)
        with self.assertRaises(UnicodeEncodeError):
            asyncio.run(run(str(self.path), "a", "\ud800"))
        self.assertEqual(self.path.read_bytes(), data)

    def test_randomized_invariant(self):
        # For every successful edit: LF(result) == LF(original) with the
        # first unique occurrence of LF(old_str) replaced by LF(new_str),
        # and every byte outside the replaced span unchanged.
        pieces = ["a", "b", "c", "\r\n", "\n", "\r"]
        for seed in range(400):
            with self.subTest(seed=seed):
                rng = random.Random(seed)
                raw = "".join(
                    rng.choice(pieces) for _ in range(rng.randint(1, 12))
                )
                old = "".join(
                    rng.choice(pieces) for _ in range(rng.randint(0, 4))
                )
                new = "".join(
                    rng.choice(pieces) for _ in range(rng.randint(0, 4))
                )
                self.path.write_bytes(raw.encode())
                expected = _lf_edit(raw, old, new)
                try:
                    asyncio.run(run(str(self.path), old, new))
                except ValueError:
                    # Not found / not unique / empty: the file must be intact.
                    self.assertIsNone(expected)
                    self.assertEqual(self.path.read_bytes(), raw.encode())
                    continue
                self.assertIsNotNone(expected)
                self.assertEqual(_to_lf(self.path.read_bytes().decode()), expected)
                self._assert_span_bytes_unchanged(raw, old)

    def _assert_span_bytes_unchanged(self, raw: str, old: str) -> None:
        # The raw span of the first LF-view occurrence: bytes before and
        # after it must be byte-identical in the result.
        needle = _to_lf(old)
        result = self.path.read_bytes().decode()
        boundaries = [0]
        index = 0
        while index < len(raw):
            index += 2 if raw[index : index + 2] == "\r\n" else 1
            boundaries.append(index)
        span: tuple[int, int] | None = None
        for start in boundaries[:-1]:
            for end in boundaries:
                if end > start and _to_lf(raw[start:end]) == needle:
                    span = (start, end)
                    break
            if span is not None:
                break
        assert span is not None
        start, end = span
        self.assertTrue(result.startswith(raw[:start]))
        self.assertTrue(result.endswith(raw[end:]))


def _to_lf(text: str) -> str:
    return text.replace("\r\n", "\n").replace("\r", "\n")


def _lf_edit(raw: str, old: str, new: str) -> str | None:
    """LF-level expectation, or None when run() must refuse the edit."""
    needle = _to_lf(old)
    if not needle:
        return None
    content = _to_lf(raw)
    first = content.find(needle)
    if first < 0 or content.find(needle, first + 1) >= 0:
        return None
    return content[:first] + _to_lf(new) + content[first + len(needle) :]


if __name__ == "__main__":
    unittest.main()
