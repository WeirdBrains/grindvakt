#!/usr/bin/env python3
# Canonical release entrypoint — Python implementation of the former release.sh
#
# Behavior parity:
# - Default bump is MINOR; --patch and --major are mutually exclusive; --to X.Y.Z targets exact SemVer.
# - --dry-run performs read-only validations (no file edits, commits, tags, or pushes) and exits early.
# - --resume safely resumes a previous attempt (see docs/RELEASES.md for exact invariants).
# - Strict safety checks: clean tree, on main tracking origin/main, monotonic version, atomic push, post-verify.
# - Updates Cargo.toml [package] version, the root package entry in Cargo.lock,
#   the pubspec.yaml version, and the version link in lib/app.dart, then runs gates:
#   fmt, clippy, test, build (only on non-dry-run new releases).
#
# NOTE: This script uses only the Python standard library.

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

try:
    import tomllib  # Python 3.11+
except Exception:
    print("error: Python 3.11+ required (tomllib)", file=sys.stderr)
    sys.exit(2)


RE_SEMVER = re.compile(r"^([1-9][0-9]*|0)\.([1-9][0-9]*|0)\.([1-9][0-9]*|0)\Z")

# Rewritten on a fresh release and restored together if a pre-commit step fails.
# Cargo.lock may be absent; the others are required when a release is cut.
RELEASE_VERSION_FILES = (
    "Cargo.toml",
    "Cargo.lock",
    "pubspec.yaml",
    "lib/app.dart",
)

# Single source for the hero version control. The anchor href and label both
# interpolate $appVersion, so the link always targets the current release tag.
RE_APP_VERSION = re.compile(r"^const String appVersion = '([0-9]+\.[0-9]+\.[0-9]+)';\s*$", re.M)
RE_APP_VERSION_LINK = re.compile(
    r"href:\s*'https://github\.com/WeirdBrains/grindvakt/releases/tag/v\$appVersion'\s*,\s*"
    r"classes:\s*'btn version'\s*,\s*\[\s*\.text\('v\$appVersion'\)",
    re.S,
)


def is_semver(s: str) -> bool:
    return bool(RE_SEMVER.match(s))


def ver_gt(a: str, b: str) -> bool:
    ax, ay, az = (int(x) for x in a.split("."))
    bx, by, bz = (int(x) for x in b.split("."))
    if ax != bx:
        return ax > bx
    if ay != by:
        return ay > by
    return az > bz


def repo_root() -> Path:
    out = run(["git", "rev-parse", "--show-toplevel"], capture_output=True).stdout.strip()
    return Path(out)


def run(cmd: list[str], *, capture_output: bool = False, check: bool = True, cwd: Path | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, text=True, capture_output=capture_output, check=check, cwd=str(cwd) if cwd else None)


def read_cur_version(root: Path) -> str:
    cargo_toml = root / "Cargo.toml"
    data = tomllib.loads(cargo_toml.read_text(encoding="utf-8"))
    return data["package"]["version"]


def set_version_in_cargo_toml(root: Path, new_version: str) -> None:
    path = root / "Cargo.toml"
    text = path.read_text(encoding="utf-8")
    # Replace only the first version line inside the [package] section, preserving formatting.
    m = re.search(r"(?ms)^\[package\]\s.*?(^version\s*=\s*\"([0-9]+\.[0-9]+\.[0-9]+)\"\s*$)", text)
    if not m:
        raise RuntimeError("failed to locate [package] version in Cargo.toml")
    start = m.start(0)
    pre = text[:start]
    pkg = text[start:]
    pkg2 = re.sub(
        r'^(version\s*=\s*")([0-9]+\.[0-9]+\.[0-9]+)(")$',
        r"\g<1>" + new_version + r"\3",
        pkg,
        count=1,
        flags=re.M,
    )
    path.write_text(pre + pkg2, encoding="utf-8")


def set_root_version_in_lock(root: Path, new_version: str) -> None:
    lock = (root / "Cargo.lock").read_text(encoding="utf-8")
    # Update only the root package entry
    pat = re.compile(r'(?ms)^(\[\[package\]\]\s*name = "grindvakt"\s*version = ")([0-9]+\.[0-9]+\.[0-9]+)(")')
    if not pat.search(lock):
        raise RuntimeError("failed to locate root package in Cargo.lock")
    new_lock = pat.sub(r"\g<1>" + new_version + r"\3", lock, count=1)
    (root / "Cargo.lock").write_text(new_lock, encoding="utf-8")


def set_version_in_pubspec(root: Path, new_version: str) -> None:
    path = root / "pubspec.yaml"
    text = path.read_text(encoding="utf-8")
    new_text, n = re.subn(
        r"^(version:\s*)([0-9]+\.[0-9]+\.[0-9]+)([^\n]*)$",
        r"\g<1>" + new_version + r"\3",
        text,
        count=1,
        flags=re.M,
    )
    if n != 1:
        raise RuntimeError("failed to locate version in pubspec.yaml")
    path.write_text(new_text, encoding="utf-8")


def read_pubspec_version(root: Path) -> str | None:
    path = root / "pubspec.yaml"
    if not path.is_file():
        return None
    m = re.search(r"^version:\s*([0-9]+\.[0-9]+\.[0-9]+)\b", path.read_text(encoding="utf-8"), re.M)
    return m.group(1) if m else None


def set_version_in_app_dart(root: Path, new_version: str) -> None:
    path = root / "lib" / "app.dart"
    text = path.read_text(encoding="utf-8")
    if not RE_APP_VERSION_LINK.search(text):
        raise RuntimeError("failed to locate version link in lib/app.dart")
    new_text, n = re.subn(
        r"^(const String appVersion = ')([0-9]+\.[0-9]+\.[0-9]+)(';)\s*$",
        r"\g<1>" + new_version + r"\3",
        text,
        count=1,
        flags=re.M,
    )
    if n != 1:
        raise RuntimeError("failed to locate appVersion in lib/app.dart")
    path.write_text(new_text, encoding="utf-8")


def read_app_dart_version(root: Path) -> str | None:
    path = root / "lib" / "app.dart"
    if not path.is_file():
        return None
    text = path.read_text(encoding="utf-8")
    if not RE_APP_VERSION_LINK.search(text):
        return None
    m = RE_APP_VERSION.search(text)
    return m.group(1) if m else None


def ensure_clean_main_tracking(root: Path) -> None:
    # Clean working tree
    if run(["git", "diff", "--name-only"], capture_output=True, cwd=root).stdout.strip():
        die("unstaged changes present")
    # On main
    if run(["git", "rev-parse", "--abbrev-ref", "HEAD"], capture_output=True, cwd=root).stdout.strip() != "main":
        die("not on main")
    # Tracking origin/main
    upstream = run(
        ["git", "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        capture_output=True,
        cwd=root,
        check=False,
    ).stdout.strip()
    if upstream != "origin/main":
        die("upstream must be origin/main")


def git_ls_remote(heads_or_tags: list[str], ref: str, root: Path) -> str:
    # heads_or_tags: ["--heads"] or ["--tags"]
    cp = run(["git", "ls-remote"] + heads_or_tags + ["origin", ref], capture_output=True, cwd=root, check=False)
    # Return the first SHA (single ref queried)
    line = cp.stdout.strip().splitlines()
    if not line:
        return ""
    return line[0].split()[0]


def early_idempotency_exit_if_released(root: Path, head: str, remote_main: str, *, skip_if_staged: bool) -> None:
    if skip_if_staged:
        # If staged changes exist, do not treat as idempotent even if HEAD is already released
        staged = run(["git", "diff", "--name-only", "--cached"], capture_output=True, cwd=root).stdout.strip()
        if staged:
            return
    # If an annotated vX.Y.Z tag points at HEAD locally, and the remote peeled commit equals HEAD,
    # and the remote annotated tag object id matches the local, and origin/main equals HEAD, exit 0.
    cp = run(["git", "tag", "--points-at", head, "--format=%(refname:short)"], capture_output=True, cwd=root)
    head_tag = ""
    for t in cp.stdout.strip().splitlines():
        if re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", t or ""):
            head_tag = t
            break
    if not head_tag:
        return
    # Must be annotated
    typ = run(["git", "cat-file", "-t", head_tag], capture_output=True, cwd=root, check=False).stdout.strip()
    if typ != "tag":
        return
    r_tag_commit = git_ls_remote(["--tags"], f"refs/tags/{head_tag}^{{}}", root)
    r_tag_obj = git_ls_remote(["--tags"], f"refs/tags/{head_tag}", root)
    l_tag_obj = run(["git", "rev-parse", f"{head_tag}^{{tag}}"], capture_output=True, cwd=root, check=False).stdout.strip()
    if r_tag_commit and remote_main == head and r_tag_obj and l_tag_obj and r_tag_obj == l_tag_obj:
        info(f"already released {head_tag}")
        sys.exit(0)


def highest_tag_version(root: Path) -> str:
    maxv = "0.0.0"
    # Remote
    cp = run(["git", "ls-remote", "--tags", "--refs", "origin", "v[0-9]*.[0-9]*.[0-9]*"], capture_output=True, cwd=root)
    for line in cp.stdout.splitlines():
        ref = line.split()[1]
        v = ref.split("/")[-1].lstrip("v")
        if is_semver(v) and ver_gt(v, maxv):
            maxv = v
    # Local
    cp2 = run(["git", "tag", "-l", "v[0-9]*.[0-9]*.[0-9]*"], capture_output=True, cwd=root)
    for t in cp2.stdout.split():
        v = t.lstrip("v")
        if is_semver(v) and ver_gt(v, maxv):
            maxv = v
    return maxv


def die(msg: str) -> None:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def info(msg: str) -> None:
    print(f"info: {msg}")


@dataclass
class Args:
    dry_run: bool
    resume: bool
    mode: str  # "minor" default, or "patch"/"major"
    to: str | None


def parse_args(argv: list[str]) -> Args:
    p = argparse.ArgumentParser(
        prog="scripts/release/release.py",
        add_help=True,
        description="Grindvakt release automation",
    )
    p.add_argument("--dry-run", action="store_true")
    p.add_argument("--resume", action="store_true")
    g = p.add_mutually_exclusive_group()
    g.add_argument("--patch", action="store_true")
    g.add_argument("--major", action="store_true")
    p.add_argument("--to", metavar="X.Y.Z")
    ns = p.parse_args(argv)
    mode = "minor"
    if ns.patch:
        mode = "patch"
    if ns.major:
        mode = "major"
    if (ns.to is not None) and (ns.patch or ns.major):
        die("use either --to or a bump flag, not both")
    return Args(dry_run=ns.dry_run, resume=ns.resume, mode=mode, to=ns.to)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    root = repo_root()
    os.chdir(root)
    ensure_clean_main_tracking(root)
    remote_main = git_ls_remote(["--heads"], "refs/heads/main", root)
    if not remote_main:
        die("cannot query origin/main")
    head = run(["git", "rev-parse", "HEAD"], capture_output=True, cwd=root).stdout.strip()
    # Fresh-path idempotency: no-op iff already fully released and no staged changes
    if not args.resume:
        early_idempotency_exit_if_released(root, head, remote_main, skip_if_staged=True)
    if args.resume:
        # Resume validations
        if run(["git", "diff", "--name-only"], capture_output=True, cwd=root).stdout.strip():
            die("resume: unstaged changes present")
        if run(["git", "diff", "--name-only", "--cached"], capture_output=True, cwd=root).stdout.strip():
            die("resume: staged changes present")
        # Idempotency exit after invariants: completed release no-ops (even in resume)
        early_idempotency_exit_if_released(root, head, remote_main, skip_if_staged=True)
        parent = run(["git", "rev-parse", "--verify", "--quiet", "HEAD^"], capture_output=True, cwd=root).stdout.strip()
        if not parent:
            die("resume requires a parent commit (HEAD^ missing)")
        if parent != remote_main:
            die("resume requires HEAD^ == origin/main")
    cur = read_cur_version(root)
    if not is_semver(cur):
        die(f"invalid Cargo.toml version {cur}")
    if args.resume:
        tag = run(["git", "tag", "--points-at", "HEAD", "--format=%(refname:short)"], capture_output=True, cwd=root).stdout
        tag = next((t for t in tag.split() if re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", t)), "")
        if tag:
            typ = run(["git", "cat-file", "-t", tag], capture_output=True, cwd=root, check=False).stdout.strip()
            if typ != "tag":
                die("resume: tag must be annotated")
            new = tag.lstrip("v")
            # Preflight: if a remote tag with the same name exists and differs in object id, refuse
            r_tag_obj = git_ls_remote(["--tags"], f"refs/tags/{tag}", root)
            l_tag_obj = run(["git", "rev-parse", f"{tag}^{{tag}}"], capture_output=True, cwd=root, check=False).stdout.strip()
            if r_tag_obj and l_tag_obj and r_tag_obj != l_tag_obj:
                die(f"resume: remote {tag} exists with different object")
        else:
            new = cur
        if not is_semver(new):
            die("resume: current Cargo.toml version invalid")
        subj = run(["git", "log", "-1", "--pretty=%s"], capture_output=True, cwd=root).stdout.strip()
        if subj != f"release: v{new}":
            die(f"resume: HEAD is not a matching release commit (expected subject 'release: v{new}')")
        # If tag exists, Cargo.toml must match
        if tag and new != cur:
            die(f"resume: Cargo.toml {cur} != tag {new}")
        # Validate Cargo.lock root version equals expected version
        lock = (root / "Cargo.lock").read_text(encoding="utf-8") if (root / "Cargo.lock").exists() else ""
        m = re.search(r'(?ms)^\[\[package\]\]\s*name = "grindvakt"\s*version = "([0-9]+\.[0-9]+\.[0-9]+)"', lock)
        if not (m and m.group(1) == new):
            die("resume: Cargo.lock root package version does not match Cargo.toml/release (%s)" % new)
        if read_pubspec_version(root) != new:
            die("resume: pubspec.yaml version does not match Cargo.toml/release (%s)" % new)
        if read_app_dart_version(root) != new:
            die("resume: lib/app.dart version link does not match Cargo.toml/release (%s)" % new)
        # Monotonic safety even on resume
        maxv = "0.0.0"
        cp_r = run(["git", "ls-remote", "--tags", "--refs", "origin", "v[0-9]*.[0-9]*.[0-9]*"], capture_output=True, cwd=root)
        for line in cp_r.stdout.splitlines():
            v = line.split()[1].split("/")[-1].lstrip("v")
            if v != new and is_semver(v) and ver_gt(v, maxv):
                maxv = v
        cp_l = run(["git", "tag", "-l", "v[0-9]*.[0-9]*.[0-9]*"], capture_output=True, cwd=root)
        for t in cp_l.stdout.split():
            v = t.lstrip("v")
            if v != new and is_semver(v) and ver_gt(v, maxv):
                maxv = v
        if not ver_gt(new, maxv):
            die(f"resume: target {new} <= highest existing {maxv}")
        if not tag:
            # Ensure no local/remote tag exists
            if run(["git", "show-ref", "--tags", "-q", "--verify", f"refs/tags/v{new}"], cwd=root, check=False).returncode == 0:
                die("resume: local v%s already exists" % new)
            if git_ls_remote(["--tags", "--refs"], f"v{new}", root):
                die("resume: remote v%s already exists" % new)
            if args.dry_run:
                print(f"DRY RUN: would create missing annotated tag v{new} and push atomically")
                return 0
            run(["git", "tag", "-a", f"v{new}", "-m", f"v{new}"], cwd=root)
            tag = f"v{new}"
    else:
        # Fresh release
        if args.to is not None:
            if args.to == "":
                die("target must be X.Y.Z")
            new = args.to  # do not normalize; reject any surrounding whitespace/newlines via is_semver
        else:
            a, b, c = (int(x) for x in cur.split("."))
            if args.mode == "patch":
                new = f"{a}.{b}.{c+1}"
            elif args.mode == "major":
                new = f"{a+1}.0.0"
            else:
                new = f"{a}.{b+1}.0"
        if not is_semver(new):
            die("target must be X.Y.Z")
        # Fresh guard: local HEAD must equal remote main (in sync) like baseline
        if head != remote_main:
            die("not in sync with origin/main")
        # Prevent downgrading manifest via --to below current manifest version
        if ver_gt(cur, new):
            die(f"target {new} < current manifest {cur}")
        # Strict monotonic: new must be > max(current manifest, highest tag)
        maxv = cur
        tagmax = highest_tag_version(root)
        if ver_gt(tagmax, maxv):
            maxv = tagmax
        if not ver_gt(new, maxv):
            die(f"target {new} <= highest {maxv}")
        # Ensure no existing local/remote tag
        if run(["git", "show-ref", "--tags", "-q", "--verify", f"refs/tags/v{new}"], cwd=root, check=False).returncode == 0:
            die("local v%s exists" % new)
        if git_ls_remote(["--tags", "--refs"], f"v{new}", root):
            die("remote v%s exists" % new)
        if args.dry_run:
            print(f"DRY RUN: would bump to {new} and push v{new} atomically")
            return 0
        # Prepare backups for full rollback on gate failures
        bakdir = Path(tempfile.mkdtemp(prefix="grindvakt-release-"))
        committed = False
        try:
            # Backup working versions and their index entries
            orig_bytes, index_entries = _backup_version_files(root, bakdir)
            try:
                # Write versions
                set_version_in_cargo_toml(root, new)
                # Update lock root package version to match manifest
                try:
                    set_root_version_in_lock(root, new)
                except Exception:
                    # Restore if update fails
                    _restore_files_and_index(root, bakdir, index_entries)
                    die("lock root update failed")
                try:
                    set_version_in_pubspec(root, new)
                    set_version_in_app_dart(root, new)
                except Exception:
                    _restore_files_and_index(root, bakdir, index_entries)
                    die("pubspec.yaml or lib/app.dart version update failed")
                # Gates (short-circuit on first failure)
                for gate in (
                    ["cargo", "fmt", "--all", "--check"],
                    ["cargo", "clippy", "--workspace", "--all-targets", "--all-features", "--locked", "--", "-D", "warnings"],
                    ["cargo", "test", "--workspace", "--all-features", "--locked"],
                    ["cargo", "build", "--release", "--locked"],
                ):
                    if run(gate, cwd=root, capture_output=True, check=False).returncode != 0:
                        _restore_files_and_index(root, bakdir, index_entries)
                        die("gates failed")
                # Stage and commit
                if run(["git", "add", *RELEASE_VERSION_FILES], cwd=root, check=False).returncode != 0:
                    _restore_files_and_index(root, bakdir, index_entries)
                    die("git add failed")
                if run(["git", "commit", "-m", f"release: v{new}"], cwd=root, check=False).returncode != 0:
                    _restore_files_and_index(root, bakdir, index_entries)
                    die("commit failed (pre-commit hook or other error)")
                committed = True
                # Create annotated tag
                if run(["git", "tag", "-a", f"v{new}", "-m", f"v{new}"], cwd=root, check=False).returncode != 0:
                    die("tag creation failed. You can resume safely with: scripts/release/release.py --resume")
            except Exception:
                # Any pre-commit subprocess failure must restore files/index and propagate
                if not committed:
                    # Write in-memory originals first to guarantee restoration even if bakdir is gone
                    _restore_bytes(root, orig_bytes)
                    _restore_files_and_index(root, bakdir, index_entries)
                raise
        finally:
            shutil.rmtree(bakdir, ignore_errors=True)
    # Push atomically (both paths)
    tag = f"v{new}"
    if args.dry_run:
        print(f"DRY RUN: would push {tag} atomically")
        return 0
    # Use a one-shot to capture stderr for remote atomic support message
    cp = run(["git", "push", "--atomic", "origin", "HEAD:refs/heads/main", f"refs/tags/{tag}"], capture_output=True, check=False, cwd=root)
    if cp.returncode != 0:
        out = (cp.stdout or "") + (cp.stderr or "")
        if re.search(r"unknown option.*atomic|does not support --atomic", out, re.I):
            die("remote does not support --atomic; refusing")
        print(out, file=sys.stderr, end="")
        die("atomic push failed")
    # Post-verify
    r_main = git_ls_remote(["--heads"], "refs/heads/main", root)
    r_tag = git_ls_remote(["--tags", "--refs"], tag, root)
    l_head = run(["git", "rev-parse", "HEAD"], capture_output=True, cwd=root).stdout.strip()
    l_tag = run(["git", "rev-parse", tag], capture_output=True, cwd=root).stdout.strip()
    if r_main != l_head:
        die("post-push branch mismatch")
    if r_tag != l_tag:
        die("post-push tag mismatch")
    info(f"done: {tag}")
    return 0


def _bak_name(rel: str) -> str:
    return rel.replace("/", "__") + ".bak"


def _backup_version_files(root: Path, bakdir: Path) -> tuple[dict[str, bytes | None], dict[str, str]]:
    orig: dict[str, bytes | None] = {}
    index_entries: dict[str, str] = {}
    for rel in RELEASE_VERSION_FILES:
        path = root / rel
        index_entries[rel] = run(["git", "ls-files", "-s", "--", rel], capture_output=True, cwd=root).stdout
        if path.is_file():
            orig[rel] = path.read_bytes()
            shutil.copy2(path, bakdir / _bak_name(rel))
        else:
            orig[rel] = None
    return orig, index_entries


def _restore_bytes(root: Path, orig: dict[str, bytes | None]) -> None:
    for rel, data in orig.items():
        dest = root / rel
        if data is None:
            try:
                dest.unlink()
            except FileNotFoundError:
                pass
        else:
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_bytes(data)


def _restore_files_and_index(root: Path, bakdir: Path, index_entries: dict[str, str]) -> None:
    for rel, idx in index_entries.items():
        bak = bakdir / _bak_name(rel)
        dest = root / rel
        if bak.exists():
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(bak, dest)
        else:
            try:
                dest.unlink()
            except FileNotFoundError:
                pass
        if idx.strip():
            mode, sha = idx.split()[0:2]
            run(["git", "update-index", "--cacheinfo", mode, sha, rel], cwd=root, check=False)
        else:
            run(["git", "restore", "--staged", "--", rel], cwd=root, check=False)


def _try(cmd: list[str], root: Path) -> int:
    cp = run(cmd, cwd=root, capture_output=True, check=False)
    return 0 if cp.returncode == 0 else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

