"""The edit skill against real files: byte-exact line endings and errors.

Run from the skill directory with ``src`` on ``PYTHONPATH``; the path is
also inserted here so a bare ``unittest discover`` works.
"""

import asyncio
import os
import sys
import tempfile
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


if __name__ == "__main__":
    unittest.main()
