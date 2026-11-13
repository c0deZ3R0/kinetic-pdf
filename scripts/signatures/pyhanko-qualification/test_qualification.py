# Developer-only regression checks for the optional pyHanko spike launcher.
#
# PATH: scripts/signatures/pyhanko-qualification/test_qualification.py
# CONSUMED BY: Python unittest; no pyHanko install or identities required.

import importlib.util
import os
from pathlib import Path
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "qualify_pyhanko.py"
spec = importlib.util.spec_from_file_location("qualify_pyhanko", SCRIPT)
qualification = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qualification)


class QualificationTests(unittest.TestCase):
    def test_pinned_release_and_artifact_hash_agree_with_install_lock(self):
        lock = (Path(__file__).parent / "requirements.txt").read_text()
        self.assertIn("pyhanko-" + qualification.VERSION + "-py3-none-any.whl", lock)
        self.assertIn(qualification.WHEEL_SHA256, lock)
        self.assertEqual(len(qualification.SOURCE_SHA256), 64)

    def test_synthetic_byte_range_reconstruction(self):
        self.assertEqual(qualification.signed_content(b"abcd<0011>efghi", [0, 4, 10, 5]), b"abcdefghi")

    def test_bad_ranges_fail_before_permissive_slicing(self):
        for ranges in ([0, 4, 10], [True, 4, 10, 5], [1, 4, 10, 5], [0, -1, 10, 5],
                       [0, 0, 10, 5], [0, 4, 4, 5], [0, 4, 3, 5], [0, 4, 10, -1], [0, 4, 10, 6]):
            with self.subTest(ranges=ranges), self.assertRaises(RuntimeError):
                qualification.signed_content(b"abcd<0011>efghi", ranges)
        with self.assertRaises(RuntimeError):
            qualification.signed_content(b"abcdNOTHEXefghi", [0, 4, 10, 5])

    def test_private_creation_does_not_overwrite_existing_files(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "synthetic.txt"
            qualification.private_write(path, b"synthetic nonsecret test data")
            with self.assertRaises(FileExistsError):
                qualification.private_write(path, b"overwrite")
            self.assertEqual(path.read_bytes(), b"synthetic nonsecret test data")
            if os.name != "nt":
                self.assertEqual(path.stat().st_mode & 0o777, 0o600)


if __name__ == "__main__":
    unittest.main()
