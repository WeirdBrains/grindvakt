# Releasing Grindvakt

Releases use the shared WeirdBrains release script, vendored from
[weirdnet-project-helpers](https://github.com/WeirdBrains/weirdnet-project-helpers) as
`scripts/weirdnet_release.py` and configured by `release.toml`. Don't edit the vendored copy:
change it upstream, re-vendor it with `vendor.py` and update the `helpers` pin.

```bash
python3 scripts/weirdnet_release.py validate          # check release.toml; change nothing
python3 scripts/weirdnet_release.py --patch --dry-run # print every step and diff
python3 scripts/weirdnet_release.py --patch --yes     # or --minor (the default), --major, --version X.Y.Z
python3 scripts/weirdnet_release.py assets X.Y.Z      # after the Release workflow: all installers attached
```

Build in a scratch checkout (set `CARGO_TARGET_DIR` outside the repo), on a clean `main` that is
pushed and up to date. Write the release notes under `## [Unreleased]` in `CHANGELOG.md` before
cutting; the script turns that heading into `## [X.Y.Z] - YYYY-MM-DD`.

## grindvakt-core lockstep

The app is built on [grindvakt-core](https://crates.io/crates/grindvakt-core)
([repo](https://github.com/WeirdBrains/grindvakt-core)). Their versions move together: Grindvakt
vX.Y.Z depends on `grindvakt-core = "=X.Y.Z"` from crates.io, never a path, git or `[patch]`
override. Every app release needs the matching crate published first, even when the crate did
not change:

1. In grindvakt-core: write its `## [Unreleased]` notes and run
   `python3 scripts/weirdnet_release.py --version X.Y.Z --yes` (same script, its own
   `release.toml`). It runs the crate's gates, commits `release: vX.Y.Z`, tags, pushes code and
   tag in one atomic push, then runs `cargo publish`.
2. In this repo: run the script with the bump that lands on X.Y.Z. The `[[requires]]` entry for
   grindvakt-core makes it ask the crates.io index for X.Y.Z before touching any file and stop if
   it is missing or yanked, or if Cargo.toml takes the crate from a path, git, another registry
   or a `[patch]`/`[replace]` override. It pins `grindvakt-core = "=X.Y.Z"`, runs
   `cargo update -p grindvakt-core --precise X.Y.Z`, and stops unless Cargo.lock then has exactly
   that version from crates.io with the index checksum.

## What a release does

1. Preflight: on `main`, clean tree, not behind `origin/main`, tag absent locally and on origin,
   version newer than every tag. Explicit versions must be plain SemVer (`X.Y.Z`).
2. The lockstep check above.
3. Bumps the version in `Cargo.toml`, the root entry of `Cargo.lock`, `pubspec.yaml` and
   `appVersion` in `lib/app.dart` (the site's version link is built from it), pins
   grindvakt-core, refreshes the lock and moves the changelog section.
4. Gates, in order: `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`,
   `cargo test --workspace --all-features --locked`, `cargo build --release --locked`.
   Any failure before the commit restores the starting commit.
5. Commit `release: vX.Y.Z`, annotated tag `vX.Y.Z`, then
   `git push --atomic origin HEAD:refs/heads/main refs/tags/vX.Y.Z`: code and tag land together
   or not at all. Tags are never moved or overwritten. If the push fails, the commit and tag stay
   local and the script prints the exact push to retry.
6. The pushed tag starts `release.yml`, which builds the Linux (musl tar.gz, deb, rpm, Arch),
   macOS (Apple Silicon .pkg) and Windows (.msi) installers, checks all nine assets and their
   checksums, and attaches them to the GitHub release. `assets X.Y.Z` checks the release against
   the `[[platforms]]` list in `release.toml`.

## WiX 7 licence

The Windows MSI is built with WiX Toolset 7, whose binaries are distributed under the Open Source
Maintenance Fee EULA v1.1 (https://docs.firegiant.com/wix/osmf/). **Michael accepted the WiX 7
OSMF EULA for Grindvakt on 2026-11-12.** On that basis `make-msi.ps1` and `release.yml` pass
`-acceptEula wix7` to every `wix` command. Revisit this if the EULA's terms or the project's
revenue change (the EULA asks no fee below US$10,000 a year).

The old `scripts/release/release.py` (and its `--resume`) was replaced by the shared script in
1.2.0; a failed push is retried with the printed command instead.

## Workflows

`release.yml` (on `v*` tags or manual dispatch) and `pages.yml` (on pushes to main) pin every
action to a full commit SHA with its version in a comment, check out without persisted
credentials, and grant each job only the permissions it needs; only the final `release` job can
write. The repository's default `GITHUB_TOKEN` is read-only, SHA pinning is required, and the
allowed actions are GitHub-owned actions plus exactly `dart-lang/setup-dart@<sha>` and
`softprops/action-gh-release@<sha>`. When you bump one of those two, update the allowed-actions
list in the repository settings to the new SHA in the same change.
