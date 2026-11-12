import contextlib
import importlib.util
import io
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

# Resolve the candidate script relative to this test file — no absolute paths.
TEST_ROOT = Path(__file__).resolve().parent
SCRIPT = TEST_ROOT.parent / "scripts" / "release" / "release.py"
spec = importlib.util.spec_from_file_location("candidate_release", SCRIPT)
mod = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = mod
spec.loader.exec_module(mod)  # type: ignore

H = "1" * 40
N = "2" * 40
T = "3" * 40
OLD = "4" * 40
CONFLICT = "5" * 40


class Fixture:
    def __init__(self, root: Path):
        self.root = root
        self.head = H
        self.parent = OLD
        self.remote = H
        self.branch = "main"
        self.upstream = "origin/main"
        self.subject = "fixture"
        self.staged = ["other.txt"]
        self.unstaged = ""
        self.current_tag = None
        self.local = {"v0.4.2": OLD}
        self.remote_tags = {"v0.4.2": (OLD, OLD)}
        self.calls = []
        self.fail = None
        self.index = {
            "Cargo.toml": ("100644", "a" * 40),
            "Cargo.lock": ("100644", "b" * 40),
            "pubspec.yaml": ("100644", "e" * 40),
            "lib/app.dart": ("100644", "f" * 40),
            "other.txt": ("100644", "c" * 40),
        }
        self.post_mismatch = False
        # Fake crates.io: every grindvakt-core version is published unless listed here.
        self.core_unpublished = set()
        self.core_yanked = set()
        self.crates_io_calls = []
        self.write_version("0.4.2")

    @staticmethod
    def core_checksum(v: str) -> str:
        return (v.replace(".", "") * 64)[:64]

    def crates_io(self, v: str):
        self.crates_io_calls.append(v)
        if v in self.core_unpublished:
            return None
        return {"num": v, "checksum": self.core_checksum(v), "yanked": v in self.core_yanked}

    def write_core_lock(self, v: str, checksum: str | None = None) -> None:
        lock = self.root / "Cargo.lock"
        text = lock.read_text()
        head, _, _ = text.partition('[[package]]\nname = "grindvakt-core"')
        lock.write_text(
            head
            + '[[package]]\nname = "grindvakt-core"\nversion = "'
            + v
            + '"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "'
            + (checksum or self.core_checksum(v))
            + '"\n'
        )

    def write_version(self, v: str) -> None:
        (self.root / "Cargo.toml").write_text(
            '[package]\nname = "grindvakt"\nversion = "'
            + v
            + '"\n\n[dependencies]\ngrindvakt-core = { version = "='
            + v
            + '", features = ["vault"] }\nfoo = "1"\n'
        )
        (self.root / "Cargo.lock").write_text(
            'version = 4\n\n[[package]]\nname = "dependency"\nversion = "1.2.3"\n\n[[package]]\nname = "grindvakt"\nversion = "'
            + v
            + '"\n\n'
        )
        self.write_core_lock(v)
        (self.root / "pubspec.yaml").write_text("name: grindvakt\nversion: " + v + "\n\nenvironment:\n  sdk: ^3.10.0\n")
        app = self.root / "lib" / "app.dart"
        app.parent.mkdir(parents=True, exist_ok=True)
        app.write_text(
            "const String appVersion = '"
            + v
            + "';\n\n"
            + "a(\n"
            + "  href: 'https://github.com/WeirdBrains/grindvakt/releases/tag/v$appVersion',\n"
            + "  classes: 'btn version',\n"
            + "  [.text('v$appVersion')],\n"
            + "),\n"
        )

    def run(self, cmd, **kw):
        self.calls.append(cmd[:])
        rc = 0
        out = ""
        if cmd[0] == "cargo":
            rc = 1 if self.fail == cmd[1] else 0
            if cmd[1] == "update" and not rc:
                # cargo update -p grindvakt-core --precise X.Y.Z
                self.write_core_lock(cmd[-1], getattr(self, "lock_checksum_override", None))
        else:
            assert cmd[0] == "git", cmd
            a = cmd[1:]
            if a == ["rev-parse", "--show-toplevel"]:
                out = str(self.root)
            elif a[:1] == ["diff"]:
                out = "\n".join(self.staged) if "--cached" in a else self.unstaged
            elif a == ["rev-parse", "--abbrev-ref", "HEAD"]:
                out = self.branch
            elif a == ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]:
                out = self.upstream
            elif a == ["rev-parse", "HEAD"]:
                out = self.head
            elif a == ["rev-parse", "--verify", "--quiet", "HEAD^"]:
                out = self.parent
            elif a[0] == "rev-parse":
                out = self.local.get(a[1].split("^{")[0], "")
            elif a[0] == "ls-remote":
                pat = a[-1]
                if "--heads" in a:
                    out = self.remote + "\trefs/heads/main\n"
                elif pat == "v[0-9]*.[0-9]*.[0-9]*":
                    out = "".join(obj + "\trefs/tags/" + tag + "\n" for tag, (obj, peeled) in self.remote_tags.items())
                else:
                    peeled = pat.endswith("^{}")
                    tag = pat.removeprefix("refs/tags/").removesuffix("^{}")
                    if tag in self.remote_tags:
                        out = self.remote_tags[tag][1 if peeled else 0] + "\trefs/tags/" + tag + ("^{}" if peeled else "") + "\n"
            elif a[0] == "tag" and "--points-at" in a:
                out = self.current_tag or ""
            elif a[:2] == ["tag", "-l"]:
                out = "\n".join(self.local)
            elif a[:2] == ["tag", "-a"]:
                rc = 1 if self.fail == "tag" else 0
                if not rc:
                    self.current_tag = a[2]
                    self.local[a[2]] = T
            elif a[:2] == ["cat-file", "-t"]:
                out = "tag"
            elif a[0] == "show-ref":
                rc = 0 if a[-1].removeprefix("refs/tags/") in self.local else 1
            elif a[0] == "log":
                out = self.subject
            elif a[0] == "ls-files":
                name = a[-1]
                if name in self.index:
                    out = " ".join(self.index[name]) + " 0\t" + name + "\n"
            elif a[0] == "update-index":
                self.index[a[-1]] = (a[2], a[3])
            elif a[0] == "restore":
                self.index.pop(a[-1], None)
            elif a[0] == "add":
                rc = 1 if self.fail == "add" else 0
                if not rc:
                    for name in a[1:]:
                        self.index[name] = ("100644", "d" * 40)
            elif a[0] == "commit":
                rc = 1 if self.fail == "commit" else 0
                if not rc:
                    self.parent = self.head
                    self.head = N
                    self.subject = a[-1]
                    self.staged = []
            elif a[0] == "push":
                rc = 1 if self.fail == "push" else 0
                tag = a[-1].removeprefix("refs/tags/")
                if tag in self.remote_tags and self.remote_tags[tag][0] != self.local[tag]:
                    rc = 1
                if not rc:
                    self.remote = OLD if self.post_mismatch else self.head
                    self.remote_tags[tag] = (self.local[tag], self.head)
            else:
                raise AssertionError(cmd)
        cp = subprocess.CompletedProcess(cmd, rc, out, "")
        if kw.get("check", True) and rc:
            raise subprocess.CalledProcessError(rc, cmd, output=out)
        return cp

    def mutations(self):
        return [
            a
            for a in self.calls
            if a[0] == "cargo"
            or a[:2] in [["git", "add"], ["git", "commit"], ["git", "push"], ["git", "update-index"]]
            or a[:3] == ["git", "tag", "-a"]
        ]


class Tests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="release-python-offline-")
        self.root = Path(self.tmp.name)
        self.f = Fixture(self.root)
        self.cwd = os.getcwd()
        self.mock = patch.object(mod, "run", self.f.run)
        self.mock.start()
        self.crates = patch.object(mod, "crates_io_core_version", self.f.crates_io)
        self.crates.start()
        self.stdout = io.StringIO()
        self.stderr = io.StringIO()
        self.o = contextlib.redirect_stdout(self.stdout)
        self.e = contextlib.redirect_stderr(self.stderr)
        self.o.__enter__()
        self.e.__enter__()

    def tearDown(self):
        os.chdir(self.cwd)
        self.mock.stop()
        self.crates.stop()
        self.o.__exit__(None, None, None)
        self.e.__exit__(None, None, None)
        self.tmp.cleanup()

    def call(self, *args):
        try:
            return mod.main(list(args))
        except SystemExit as ex:
            return ex.code

    def version_bytes(self):
        names = ["Cargo.toml", "Cargo.lock", "pubspec.yaml", "lib/app.dart"]
        return {n: (self.root / n).read_bytes() for n in names}

    def resume(self, tag=True):
        self.f.write_version("0.5.0")
        self.f.head = N
        self.f.parent = H
        self.f.subject = "release: v0.5.0"
        self.f.staged = []
        if tag:
            self.f.current_tag = "v0.5.0"
            self.f.local["v0.5.0"] = T

    # A minimal subset smoke to keep portable in-tar tests
    def test_dry_run_no_mutation(self):
        self.assertEqual(self.call("--dry-run"), 0)
        self.assertFalse(self.f.mutations())
        self.assertEqual(mod.read_cur_version(self.root), "0.4.2")

    def test_parse_modes(self):
        self.assertEqual(mod.parse_args([]).mode, "minor")
        self.assertEqual(mod.parse_args(["--patch"]).mode, "patch")
        self.assertEqual(mod.parse_args(["--major"]).mode, "major")

    def test_conflicting_args_rejected(self):
        for args in [["--patch", "--major"], ["--to", "1.0.0", "--patch"], ["--minor"]]:
            with self.assertRaises(SystemExit):
                mod.parse_args(args)

    def test_semver_valid_and_invalid(self):
        for v in ["0.0.0", "10.20.30"]:
            self.assertTrue(mod.is_semver(v))
        for v in ["01.2.3", "1.2", "1.2.3-pre", "1.2.3+build", "-1.2.3"]:
            self.assertFalse(mod.is_semver(v))

    def test_version_comparison(self):
        self.assertTrue(mod.ver_gt("1.10.0", "1.9.99"))
        self.assertFalse(mod.ver_gt("1.0.0", "1.0.0"))

    def test_version_edits_only_root(self):
        mod.set_version_in_cargo_toml(self.root, "0.5.0")
        mod.set_root_version_in_lock(self.root, "0.5.0")
        self.assertEqual(mod.read_cur_version(self.root), "0.5.0")
        self.assertIn('name = "dependency"\nversion = "1.2.3"', (self.root / "Cargo.lock").read_text())

    def test_fresh_release_atomic_and_annotated(self):
        self.assertEqual(self.call(), 0)
        self.assertIn(["git", "tag", "-a", "v0.5.0", "-m", "v0.5.0"], self.f.calls)
        self.assertIn(
            ["git", "push", "--atomic", "origin", "HEAD:refs/heads/main", "refs/tags/v0.5.0"],
            self.f.calls,
        )
        self.assertEqual(mod.read_cur_version(self.root), "0.5.0")
        self.assertEqual(mod.read_pubspec_version(self.root), "0.5.0")
        self.assertEqual(mod.read_app_dart_version(self.root), "0.5.0")
        app = (self.root / "lib" / "app.dart").read_text()
        self.assertIn("releases/tag/v$appVersion", app)
        self.assertIn(".text('v$appVersion')", app)

    def test_patch_and_major(self):
        self.assertEqual(self.call("--patch", "--dry-run"), 0)
        self.assertIn("0.4.3", self.stdout.getvalue())
        self.stdout.seek(0)
        self.stdout.truncate()
        self.assertEqual(self.call("--major", "--dry-run"), 0)
        self.assertIn("1.0.0", self.stdout.getvalue())

    def test_wrong_branch_rejected(self):
        self.f.branch = "feature"
        self.assertNotEqual(self.call(), 0)
        self.assertFalse(self.f.mutations())

    def test_dirty_tree_rejected(self):
        self.f.unstaged = "dirty.txt"
        self.assertNotEqual(self.call(), 0)
        self.assertFalse(self.f.mutations())

    def test_diverged_main_rejected(self):
        self.f.remote = OLD
        self.assertNotEqual(self.call(), 0)
        self.assertFalse(self.f.mutations())

    def test_idempotency_clean(self):
        self.f.staged = []
        self.f.current_tag = "v0.4.2"
        self.f.local["v0.4.2"] = T
        self.f.remote_tags["v0.4.2"] = (T, H)
        self.assertEqual(self.call(), 0)
        self.assertIn("already released", self.stdout.getvalue())
        self.assertFalse(self.f.mutations())

    def test_staged_candidate_not_skipped_by_idempotency(self):
        self.f.current_tag = "v0.4.2"
        self.f.local["v0.4.2"] = T
        self.f.remote_tags["v0.4.2"] = (T, H)
        self.assertEqual(self.call(), 0)
        self.assertTrue(any(c[:2] == ["git", "commit"] for c in self.f.calls), "staged changes must produce a new release, not noop")

    def test_semver_rejects_trailing_newline(self):
        self.assertFalse(mod.is_semver("1.2.3\n"))

    def test_to_empty_rejected(self):
        self.assertNotEqual(self.call("--to", "", "--dry-run"), 0)

    def test_fresh_monotonic_and_sync(self):
        self.f.remote = OLD  # diverged
        self.assertNotEqual(self.call("--to", "0.5.0", "--dry-run"), 0)
        self.f.remote = H
        self.f.write_version("0.9.0")
        self.assertNotEqual(self.call("--to", "0.9.0", "--dry-run"), 0)

    def test_resume_completed_noop_but_rejects_staged(self):
        self.resume()
        self.f.remote = N
        self.f.remote_tags["v0.5.0"] = (T, N)
        self.assertEqual(self.call("--resume"), 0)
        self.f.staged = ["other.txt"]
        self.assertNotEqual(self.call("--resume"), 0)

    def test_missing_cargo_restores_files(self):
        before = self.version_bytes()
        original = self.f.run

        def missing(cmd, **kw):
            if cmd[0] == "cargo":
                raise FileNotFoundError("fixture: cargo absent")
            return original(cmd, **kw)

        with patch.object(mod, "run", missing):
            try:
                self.call()
            except FileNotFoundError:
                pass
        self.assertEqual(
            before, {n: (self.root / n).read_bytes() for n in before}, "pre-commit subprocess exception must restore version files"
        )

    def test_gates_failure_restores_files_and_index(self):
        self.f.fail = "fmt"
        before = self.version_bytes()
        index = self.f.index.copy()
        self.assertNotEqual(self.call(), 0)
        self.assertEqual(before, {n: (self.root / n).read_bytes() for n in before})
        self.assertEqual(index, self.f.index)
        self.assertFalse(any(c[:2] == ["git", "commit"] for c in self.f.calls))

    def test_gate_failure_short_circuits(self):
        self.f.fail = "fmt"
        self.assertNotEqual(self.call(), 0)
        # The grindvakt-core lock refresh runs before the gates; the gates stop at the first failure.
        self.assertEqual([c[1] for c in self.f.calls if c[0] == "cargo"], ["update", "fmt"])

    def test_commit_failure_preserves_unrelated_stage(self):
        self.f.fail = "commit"
        before = self.version_bytes()
        index = self.f.index.copy()
        self.assertNotEqual(self.call(), 0)
        self.assertEqual(before, {n: (self.root / n).read_bytes() for n in before})
        self.assertEqual(index, self.f.index)

    def test_lock_update_failure_restores_manifest(self):
        (self.root / "Cargo.lock").write_text("version = 4\n")
        before = (self.root / "Cargo.toml").read_bytes()
        self.assertNotEqual(self.call(), 0)
        self.assertEqual(before, (self.root / "Cargo.toml").read_bytes())

    def test_postverify_detects_mismatch(self):
        self.f.post_mismatch = True
        self.assertNotEqual(self.call(), 0)
        self.assertIn("post-push branch mismatch", self.stderr.getvalue())

    def test_atomic_push_failure_no_fallback(self):
        self.f.fail = "push"
        self.assertNotEqual(self.call(), 0)
        self.assertEqual(len([c for c in self.f.calls if c[:2] == ["git", "push"]]), 1)

    def test_resume_missing_tag(self):
        self.resume(False)
        self.assertEqual(self.call("--resume"), 0)
        self.assertTrue(any(c[:3] == ["git", "tag", "-a"] for c in self.f.calls))
        self.assertFalse(any(c[0] == "cargo" for c in self.f.calls))

    def test_resume_missing_tag_dry_run(self):
        self.resume(False)
        self.assertEqual(self.call("--resume", "--dry-run"), 0)
        self.assertFalse(self.f.mutations())

    def test_resume_existing_tag_dry_run(self):
        self.resume()
        self.assertEqual(self.call("--resume", "--dry-run"), 0)
        self.assertFalse(self.f.mutations(), "dry-run must never push")

    def test_resume_remote_conflicting_tag_rejected(self):
        self.resume()
        self.f.remote_tags["v0.5.0"] = (CONFLICT, OLD)
        self.assertNotEqual(self.call("--resume"), 0)
        self.assertFalse(self.f.mutations())

    def test_resume_already_completed_noop(self):
        self.resume()
        self.f.remote = N
        self.f.remote_tags["v0.5.0"] = (T, N)
        self.assertEqual(self.call("--resume"), 0)
        self.assertFalse(self.f.mutations())

    def test_resume_staged_rejected(self):
        self.resume()
        self.f.staged = ["other.txt"]
        self.assertNotEqual(self.call("--resume"), 0)
        self.assertFalse(self.f.mutations())

    def test_git_add_failure_restores_files_and_stage(self):
        self.f.fail = "add"
        before = self.version_bytes()
        index = self.f.index.copy()
        self.assertNotEqual(self.call(), 0)
        self.assertEqual(before, {n: (self.root / n).read_bytes() for n in before})
        self.assertEqual(index, self.f.index)

    def test_tag_failure_retains_release_commit_for_resume(self):
        self.f.fail = "tag"
        self.assertNotEqual(self.call(), 0)
        self.assertEqual(self.f.head, N)
        self.assertEqual(mod.read_cur_version(self.root), "0.5.0")
        self.assertFalse(any(c[:2] == ["git", "push"] for c in self.f.calls))

    def test_documented_python_entrypoint_executable(self):
        # Portable: check executable bit on the script itself
        self.assertTrue(os.access(SCRIPT, os.X_OK))

    def test_python_entrypoint_from_subdirectory(self):
        # Simulate invocation from a nested directory; run main() with patched git returning repo root
        sub = self.root / "nested"
        sub.mkdir()
        os.chdir(sub)
        self.assertEqual(self.call("--dry-run"), 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)

