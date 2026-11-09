# Release script usage

The canonical entrypoint is:

```bash
scripts/release/release.py [--dry-run] [--resume] [--patch|--major] [--to X.Y.Z]
```

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
- Version bump is written to Cargo.toml, the root package entry in Cargo.lock, the `version:` field in pubspec.yaml, and `appVersion` in lib/app.dart. The version link (`classes: 'btn version'`) builds its href and label from `$appVersion`, so it always points at `releases/tag/v` plus the current version. Gates then run in order:
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
- Dry-run is strictly read-only and does not fetch; tag/branch checks use `git ls-remote` only.
 

