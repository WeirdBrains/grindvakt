# Release script usage

The canonical entrypoint is:

```bash
scripts/release/release.py [--dry-run] [--resume] [--patch|--major] [--to X.Y.Z]
```

## grindvakt-core lockstep

The app is built on [grindvakt-core](https://crates.io/crates/grindvakt-core)
([repo](https://github.com/WeirdBrains/grindvakt-core)). Their versions move together: Grindvakt
vX.Y.Z depends on `grindvakt-core = "=X.Y.Z"` from crates.io, never a path, git or `[patch]`
override. Every app release therefore needs the matching crate published first, even when the
crate did not change:

1. In grindvakt-core: add the `## [X.Y.Z]` CHANGELOG section and run `scripts/release.py X.Y.Z`.
   It runs its gates, tags vX.Y.Z, pushes code and tag together, runs `cargo publish`, and waits
   until crates.io serves X.Y.Z with the checksum of the local `.crate`.
2. In this repo: stage the app changes and run `scripts/release/release.py` with the bump that
   lands on X.Y.Z. Before touching any file it asks crates.io for grindvakt-core X.Y.Z and stops
   if it is missing or yanked. It then pins the dependency to `"=X.Y.Z"`, runs
   `cargo update -p grindvakt-core --precise X.Y.Z`, and refuses to continue unless Cargo.lock
   has exactly that version from crates.io with the checksum crates.io reports. Only then do the
   gates, the commit, the tag and the push run.

`--resume` applies the same checks before it creates a missing tag or pushes, so an app tag is
never pushed against an unpublished or diverging crate.

Behavior

- Default bump is MINOR when no flags are provided.
- `--patch` bumps PATCH; `--major` bumps MAJOR. These flags are mutually exclusive.
- `--to X.Y.Z` targets an exact SemVer (no pre-release/build metadata). Cannot be combined with bump flags.
- `--dry-run` performs all read-only validations without mutating the repository or pushing.
- `--resume` safely resumes a previous attempt:
  - If the previous attempt created the release commit but tag creation or push failed, `--resume` will create the missing annotated tag only when it is provably safe (HEAD commit subject `release: vX.Y.Z`, HEAD^ equals origin/main, versions in Cargo.toml, Cargo.lock, pubspec.yaml, and the version link in lib/app.dart match, and no local/remote tag exists), then perform the atomic push.
  - If the release is already completed on origin (origin/main equals HEAD, and the remote annotated tag’s peeled commit equals HEAD, and the remote annotated tag object id matches the local tag object id), the script exits 0 without bumping or pushing.

Guards and gates

- Requires a clean working tree and branch `main` tracking `origin/main`.
- Monotonic version check against the highest local and remote stable tags.
- grindvakt-core X.Y.Z must be published on crates.io (not yanked); otherwise the script stops before any edit, including in `--dry-run`.
- Version bump is written to Cargo.toml (the package version and the `grindvakt-core = "=X.Y.Z"` pin), the root package entry in Cargo.lock, the `version:` field in pubspec.yaml, and `appVersion` in lib/app.dart; Cargo.lock is refreshed with `cargo update -p grindvakt-core --precise X.Y.Z` and must then match the crates.io version and checksum. The version link (`classes: 'btn version'`) builds its href and label from `$appVersion`, so it always points at `releases/tag/v` plus the current version. Gates then run in order:
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
  - `cargo test --workspace --all-features --locked`
  - `cargo build --release --locked`
- On any pre-commit failure, Cargo.toml, Cargo.lock, pubspec.yaml, and lib/app.dart are restored exactly in the working tree and index to their original state.
- The release commit has message `release: vX.Y.Z`. On commit failure (e.g., hooks), files and index are restored to their original state.
- Tag creation failures do not roll back the commit; re-run with `--resume` to safely create the missing annotated tag.
- Push uses `git push --atomic origin HEAD:refs/heads/main refs/tags/vX.Y.Z`, then verifies both branch and tag SHAs on origin match local.

Notes

- The script never moves or overwrites existing tags (local or remote).
- Dry-run is strictly read-only and does not fetch; tag/branch checks use `git ls-remote` only, and the grindvakt-core check is a read-only crates.io API request.
- Tests: `python3 -m pytest release-tests` (offline; git, cargo and crates.io are faked).
 

