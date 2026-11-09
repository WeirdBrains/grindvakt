import importlib.util
import sys
import unittest
from pathlib import Path

# Import the portable original 32-suite from this tar
p = Path(__file__).resolve().parent / "test_release_conversion.py"
spec = importlib.util.spec_from_file_location("independent_original32", p)
base = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = base
spec.loader.exec_module(base)  # type: ignore


class Additional(base.Tests):
    def test_edge_cli_to_rejects_trailing_newline(self):
        self.assertNotEqual(self.call("--to", "0.5.0\n", "--dry-run"), 0, "strict CLI SemVer must not normalize final newline")

    def test_edge_cli_to_rejects_empty(self):
        self.assertNotEqual(self.call("--to", "", "--dry-run"), 0, "explicit empty --to must not silently become default minor")

    def test_edge_manifest_equal_to_target_rejected(self):
        self.f.write_version("0.9.0")
        self.assertNotEqual(self.call("--to", "0.9.0", "--dry-run"), 0, "target must be strictly greater than both current manifest and tags")

    def test_edge_completed_resume_with_staged_work_rejected(self):
        self.resume()
        self.f.remote = base.N
        self.f.remote_tags["v0.5.0"] = (base.T, base.N)
        self.f.staged = ["other.txt"]
        self.assertNotEqual(self.call("--resume"), 0, "staged work must not bypass resume invariants via idempotency")
        self.assertFalse(self.f.mutations())

    def test_edge_postcommit_tag_spawn_error_keeps_release_files(self):
        original = self.f.run

        def unavailable(cmd, **kw):
            if cmd[:3] == ["git", "tag", "-a"]:
                raise FileNotFoundError("fixture: git executable unavailable at tag creation")
            return original(cmd, **kw)

        with unittest.mock.patch.object(base.mod, "run", unavailable):
            try:
                self.call()
            except FileNotFoundError:
                pass
        self.assertEqual(self.f.head, base.N)
        self.assertEqual(base.mod.read_cur_version(self.root), "0.5.0", "committed release must not be restored after tag spawn failure")
        self.assertIn('name = "grindvakt"\nversion = "0.5.0"', (self.root / "Cargo.lock").read_text())

    def test_edge_shipped_tests_target_local_candidate(self):
        text = (Path(__file__).resolve().parent / "test_release_conversion.py").read_text()
        self.assertNotIn("/workspace/", text, "shipped tests must resolve checked-in candidate, not a reviewer-specific path")
        self.assertNotIn("/cursor/stores/", text)


def load_tests(loader, tests, pattern):
    return unittest.TestSuite(Additional(n) for n in loader.getTestCaseNames(Additional) if n.startswith("test_edge_"))


if __name__ == "__main__":
    unittest.main(verbosity=2)

