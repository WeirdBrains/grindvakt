"""grindvakt-core lockstep: app vX.Y.Z ships only against the published grindvakt-core X.Y.Z."""
import importlib.util
import sys
import unittest
from pathlib import Path

p = Path(__file__).resolve().parent / "test_release_conversion.py"
spec = importlib.util.spec_from_file_location("lockstep_base", p)
base = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = base
spec.loader.exec_module(base)  # type: ignore
mod = base.mod

UPDATE = ["cargo", "update", "-p", "grindvakt-core", "--precise", "0.5.0"]


class Lockstep(base.Tests):
    def cargo_toml(self):
        return (self.root / "Cargo.toml").read_text()

    def test_core_fresh_release_pins_and_locks_core(self):
        self.assertEqual(self.call(), 0)
        self.assertIn(UPDATE, self.f.calls)
        self.assertIn('grindvakt-core = { version = "=0.5.0", features = ["vault"] }', self.cargo_toml())
        entry = mod.read_core_lock(self.root)
        self.assertEqual(entry["version"], "0.5.0")
        self.assertEqual(entry["checksum"], self.f.core_checksum("0.5.0"))
        self.assertIn("0.5.0", self.f.crates_io_calls)
        # Lock refresh comes before the gates, and the tag only after the gates.
        cargo = [c for c in self.f.calls if c[0] == "cargo"]
        self.assertEqual(cargo[0], UPDATE)
        tag_at = self.f.calls.index(["git", "tag", "-a", "v0.5.0", "-m", "v0.5.0"])
        self.assertLess(self.f.calls.index(UPDATE), tag_at)

    def test_core_unpublished_refuses_before_any_change(self):
        self.f.core_unpublished.add("0.5.0")
        before = self.version_bytes()
        self.assertNotEqual(self.call(), 0)
        self.assertFalse(self.f.mutations())
        self.assertEqual(before, self.version_bytes())
        self.assertIn("is not published on crates.io", self.stderr.getvalue())
        self.assertIn("release grindvakt-core 0.5.0 first", self.stderr.getvalue())

    def test_core_unpublished_refused_in_dry_run(self):
        self.f.core_unpublished.add("1.0.0")
        self.assertNotEqual(self.call("--major", "--dry-run"), 0)
        self.assertFalse(self.f.mutations())

    def test_core_published_dry_run_reports_it(self):
        self.assertEqual(self.call("--major", "--dry-run"), 0)
        self.assertIn("grindvakt-core 1.0.0 is published", self.stdout.getvalue())
        self.assertFalse(self.f.mutations())

    def test_core_yanked_refused(self):
        self.f.core_yanked.add("0.5.0")
        self.assertNotEqual(self.call(), 0)
        self.assertFalse(self.f.mutations())
        self.assertIn("yanked", self.stderr.getvalue())

    def test_core_major_release_pins_matching_version(self):
        self.assertEqual(self.call("--major"), 0)
        self.assertIn('version = "=1.0.0"', self.cargo_toml())
        self.assertEqual(mod.read_core_lock(self.root)["version"], "1.0.0")

    def test_core_checksum_mismatch_after_update_restores_files(self):
        self.f.lock_checksum_override = "0" * 64
        before = self.version_bytes()
        self.assertNotEqual(self.call(), 0)
        self.assertEqual(before, self.version_bytes())
        self.assertFalse(any(c[:2] == ["git", "commit"] for c in self.f.calls))
        self.assertIn("checksum", self.stderr.getvalue())

    def test_core_update_failure_restores_files(self):
        self.f.fail = "update"
        before = self.version_bytes()
        self.assertNotEqual(self.call(), 0)
        self.assertEqual(before, self.version_bytes())
        self.assertFalse(any(c[:2] == ["git", "commit"] for c in self.f.calls))

    def test_core_path_or_git_dependency_refused(self):
        for dep in ['{ path = "../grindvakt-core" }', '{ version = "=0.4.2", path = "../core" }', '{ git = "https://example.invalid/core" }']:
            text = self.cargo_toml()
            (self.root / "Cargo.toml").write_text(
                text.replace('grindvakt-core = { version = "=0.4.2", features = ["vault"] }', "grindvakt-core = " + dep)
            )
            self.assertNotEqual(self.call("--dry-run"), 0, dep)
            self.assertIn("must come from crates.io", self.stderr.getvalue())
            self.f.write_version("0.4.2")
        self.assertFalse(self.f.mutations())

    def test_core_patch_override_refused(self):
        (self.root / "Cargo.toml").write_text(self.cargo_toml() + '\n[patch.crates-io]\ngrindvakt-core = { path = "../core" }\n')
        self.assertNotEqual(self.call("--dry-run"), 0)
        self.assertIn("[patch] overrides grindvakt-core", self.stderr.getvalue())

    def test_core_missing_dependency_refused(self):
        text = self.cargo_toml().replace('grindvakt-core = { version = "=0.4.2", features = ["vault"] }\n', "")
        (self.root / "Cargo.toml").write_text(text)
        self.assertNotEqual(self.call("--dry-run"), 0)
        self.assertIn("must depend on grindvakt-core", self.stderr.getvalue())

    def test_core_resume_requires_matching_pin_and_lock(self):
        self.resume(tag=False)
        self.f.write_core_lock("0.4.2")
        self.assertNotEqual(self.call("--resume"), 0)
        self.assertIn("Cargo.lock has grindvakt-core 0.4.2, expected 0.5.0", self.stderr.getvalue())
        self.assertFalse(self.f.mutations())

    def test_core_resume_requires_published_core(self):
        self.resume()
        self.f.core_unpublished.add("0.5.0")
        self.assertNotEqual(self.call("--resume"), 0)
        self.assertFalse(any(c[:2] == ["git", "push"] for c in self.f.calls))

    def test_core_resume_ok_when_in_lockstep(self):
        self.resume()
        self.assertEqual(self.call("--resume"), 0)
        self.assertIn(["git", "push", "--atomic", "origin", "HEAD:refs/heads/main", "refs/tags/v0.5.0"], self.f.calls)

    def test_core_requirement_parsing(self):
        self.assertEqual(mod.core_requirement_version("=1.2.3"), "1.2.3")
        self.assertEqual(mod.core_requirement_version("= 1.2.3"), "1.2.3")
        for req in ["1.2.3", "^1.2.3", "~1.2", ">=1.2.3", "=1.2", "=1.2.3-pre"]:
            self.assertIsNone(mod.core_requirement_version(req), req)

    def test_core_set_requirement_string_and_table_forms(self):
        for line, want in [
            ('grindvakt-core = "1.0.0"\n', 'grindvakt-core = "=2.0.0"\n'),
            ('grindvakt-core = "=1.0.0"\n', 'grindvakt-core = "=2.0.0"\n'),
            ('grindvakt-core = { version = "=1.0.0", features = ["vault"] }\n', 'grindvakt-core = { version = "=2.0.0", features = ["vault"] }\n'),
            ('grindvakt-core = { features = ["vault"], version = "1" }\n', 'grindvakt-core = { features = ["vault"], version = "=2.0.0" }\n'),
        ]:
            (self.root / "Cargo.toml").write_text('[package]\nname = "grindvakt"\nversion = "1.0.0"\n\n[dependencies]\n' + line)
            mod.set_core_requirement(self.root, "2.0.0")
            self.assertTrue(self.cargo_toml().endswith(want), self.cargo_toml())

    def test_core_crates_io_unreachable_refuses(self):
        self.crates.stop()
        import urllib.error

        def boom(*a, **k):
            raise urllib.error.URLError("offline")

        with unittest.mock.patch.object(mod.urllib.request, "urlopen", boom):
            self.assertNotEqual(self.call("--dry-run"), 0)
        self.crates.start()
        self.assertIn("cannot query crates.io", self.stderr.getvalue())

    def test_core_docs_describe_lockstep(self):
        docs = (Path(__file__).resolve().parent.parent / "docs" / "RELEASES.md").read_text()
        self.assertIn("grindvakt-core", docs)
        self.assertIn("=X.Y.Z", docs)


def load_tests(loader, tests, pattern):
    return unittest.TestSuite(Lockstep(n) for n in loader.getTestCaseNames(Lockstep) if n.startswith("test_core_"))


if __name__ == "__main__":
    unittest.main(verbosity=2)
