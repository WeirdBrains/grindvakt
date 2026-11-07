#!/usr/bin/env bash
set -euo pipefail
usage(){ cat <<'U'
Usage:
  scripts/release/release.sh [--dry-run] [--resume] [--patch|--major] [--to X.Y.Z]
U
}
die(){ echo "error: $*" >&2; exit 1; }
info(){ echo "info: $*"; }
is_semver(){ [[ "$1" =~ ^([1-9][0-9]*|0)\.([1-9][0-9]*|0)\.([1-9][0-9]*|0)$ ]]; }
ver_gt(){ IFS=. read -r a b c <<<"$1"; IFS=. read -r x y z <<<"$2"; (( a > x )) || { (( a == x )) && ( (( b > y )) || { (( b == y )) && (( c > z )); } ); }; }
repo_root(){ git rev-parse --show-toplevel; }
read_cur(){ python3 - <<'PY'
import tomllib
print(tomllib.load(open('Cargo.toml','rb'))['package']['version'])
PY
}
set_ver(){ python3 - "$1" <<'PY'
import sys,re
new=sys.argv[1]
text=open('Cargo.toml','r',encoding='utf-8').read()
m=re.search(r'(?ms)^\[package\]\s.*?^version\s*=\s*"(.*?)"\s*$',text)
if not m: raise SystemExit(2)
start=m.start(0); pre=text[:start]; pkg=text[start:]
pkg=re.sub(r'^(version\s*=\s*")([\d\.]+)(")$', r'\g<1>'+new+r'\3', pkg, count=1, flags=re.M)
open('Cargo.toml','w',encoding='utf-8').write(pre+pkg)
PY
}
mode=minor; dry=false; resume=false; to=""; f_patch=false; f_major=false
while [[ $# -gt 0 ]]; do case "$1" in --dry-run) dry=true;; --resume) resume=true;; --patch) mode=patch; f_patch=true;; --major) mode=major; f_major=true;; --minor) die "--minor is default";; --to) shift; to="${1:-}"; [[ -n "$to" ]] || die "--to requires version";; -h|--help) usage; exit 0;; *) die "unknown arg $1";; esac; shift; done
$f_patch && $f_major && die "cannot use --patch and --major together"
if [[ -n "$to" ]] && { [[ "$f_patch" = true ]] || [[ "$f_major" = true ]]; }; then die "use either --to or a bump flag, not both"; fi
cd "$(repo_root)" || die "not a git repo"
[[ -z "$(git diff --name-only)" ]] || die "unstaged changes present"
[[ "$(git rev-parse --abbrev-ref HEAD)" == "main" ]] || die "not on main"
[[ "$(git rev-parse --abbrev-ref --symbolic-full-name @{u} 2>/dev/null)" == "origin/main" ]] || die "upstream must be origin/main"
remote_main=$(git ls-remote --heads origin refs/heads/main | awk '{print $1}')
[[ -n "$remote_main" ]] || die "cannot query origin/main"
head=$(git rev-parse HEAD)
#
# Idempotency: if this exact HEAD is already released on origin (annotated tag whose peeled commit equals HEAD,
# and origin/main also equals HEAD), and the remote annotated tag object matches the local annotated tag object,
# and there are no staged changes, exit 0 early.
staged_changes=$(git diff --name-only --cached)
if [[ -z "$staged_changes" ]]; then
  head_tag=$(git tag --points-at HEAD --format='%(refname:short)' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' || true)
  if [[ -n "$head_tag" ]]; then
    # Require annotated, and compare peeled commit on origin to HEAD
    if [[ "$(git cat-file -t "$head_tag" 2>/dev/null)" = "tag" ]]; then
      # Remote peeled commit and tag object id using exact-ref equality (avoid regex pitfalls)
      r_tag_commit=$(git ls-remote --tags origin | awk -v ref="refs/tags/$head_tag^{}" '$2==ref{print $1}')
      r_tag_obj=$(git ls-remote --tags origin | awk -v ref="refs/tags/$head_tag" '$2==ref{print $1}')
      l_tag_obj=$(git rev-parse "$head_tag^{tag}" 2>/dev/null || true)
      if [[ -n "$r_tag_commit" && "$r_tag_commit" == "$head" && "$remote_main" == "$head" && -n "$r_tag_obj" && -n "$l_tag_obj" && "$r_tag_obj" == "$l_tag_obj" ]]; then
        info "already released $head_tag"
        exit 0
      fi
    fi
  fi
fi
# Sync state checks (after idempotency detection)
if ! $resume; then
  [[ "$head" == "$remote_main" ]] || die "not in sync with origin/main"
else
  # Resume requires a perfectly clean index/tree (no staged or unstaged tracked changes)
  [[ -z "$(git diff --name-only)" ]] || die "resume: unstaged changes present"
  [[ -z "$(git diff --name-only --cached)" ]] || die "resume: staged changes present"
  parent=$(git rev-parse --verify --quiet HEAD^ || true)
  [[ -n "$parent" ]] || die "resume requires a parent commit (HEAD^ missing)"
  [[ "$parent" == "$remote_main" ]] || die "resume requires HEAD^ == origin/main"
fi
#
cur=$(read_cur); is_semver "$cur" || die "invalid Cargo.toml version $cur"
if $resume; then
  tag=$(git tag --points-at HEAD --format='%(refname:short)' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' || true)
  if [[ -n "$tag" ]]; then
    [[ "$(git cat-file -t "$tag" 2>/dev/null)" = "tag" ]] || die "resume: tag must be annotated"
    new="${tag#v}"
  else
    new="$cur"
  fi
  # Common resume validations for both paths:
  is_semver "$new" || die "resume: current Cargo.toml version invalid"
  subj=$(git log -1 --pretty=%s)
  [[ "$subj" == "release: v$new" ]] || die "resume: HEAD is not a matching release commit (expected subject 'release: v$new')"
  # Cargo.toml must match tag (if present)
  if [[ -n "$tag" ]]; then [[ "$new" == "$cur" ]] || die "resume: Cargo.toml $cur != tag $new"; fi
  # Validate locked root package version equals expected version
  if ! python3 - "$new" <<'PY'
import sys,re
new=sys.argv[1]
lock=open('Cargo.lock','r',encoding='utf-8').read()
m=re.search(r'(?ms)^\[\[package\]\]\s*name = "grindvakt"\s*version = "([0-9]+\.[0-9]+\.[0-9]+)"', lock)
sys.exit(0 if m and m.group(1)==new else 3)
PY
  then
    die "resume: Cargo.lock root package version does not match Cargo.toml/release ($new)"
  fi
  # Enforce monotonic safety even on resume: new must be > highest existing stable tag (excluding v$new itself)
  max="0.0.0"
  while read -r sha ref; do
    t=${ref##refs/tags/}; v=${t#v}; is_semver "$v" || continue
    [[ "$v" == "$new" ]] && continue
    ver_gt "$v" "$max" && max="$v"
  done < <(git ls-remote --tags --refs origin 'v[0-9]*.[0-9]*.[0-9]*')
  for t in $(git tag -l 'v[0-9]*.[0-9]*.[0-9]*'); do
    v=${t#v}; is_semver "$v" || continue
    [[ "$v" == "$new" ]] && continue
    ver_gt "$v" "$max" && max="$v"
  done
  ver_gt "$new" "$max" || die "resume: target $new <= highest existing $max"
  if [[ -n "$tag" ]]; then
    # If a remote tag with same name exists, it must match our local annotated tag object
    r_tag_obj=$(git ls-remote --tags origin | awk -v ref="refs/tags/$tag" '$2==ref{print $1}')
    l_tag_obj=$(git rev-parse "$tag^{tag}" 2>/dev/null || true)
    if [[ -n "$r_tag_obj" && -n "$l_tag_obj" && "$r_tag_obj" != "$l_tag_obj" ]]; then
      die "resume: remote $tag exists with different object"
    fi
  else
    # Ensure no local/remote tag v$new exists, then create the missing tag (unless dry-run)
    [[ -z "$(git show-ref --tags -q --verify "refs/tags/v$new" && echo x)" ]] || die "resume: local v$new already exists"
    [[ -z "$(git ls-remote --tags --refs origin "v$new" | cat)" ]] || die "resume: remote v$new already exists"
    tag="v$new"
    if $dry; then
      echo "DRY RUN: would create missing annotated tag $tag and push atomically"
      exit 0
    fi
    git tag -a "$tag" -m "$tag" || die "resume: failed to create annotated tag $tag"
  fi
else
  if [[ -n "$to" ]]; then new="$to"; else IFS=. read -r a b c <<<"$cur"; case "$mode" in patch)new="$a.$b.$((c+1))";; minor)new="$a.$((b+1)).0";; major)new="$((a+1)).0.0";; esac; fi; is_semver "$new" || die "target must be X.Y.Z";
fi
# Highest tag
max="$cur"; while read -r sha ref; do t=${ref##refs/tags/}; v=${t#v}; is_semver "$v" || continue; ver_gt "$v" "$max" && max="$v"; done < <(git ls-remote --tags --refs origin 'v[0-9]*.[0-9]*.[0-9]*'); for t in $(git tag -l 'v[0-9]*.[0-9]*.[0-9]*'); do v=${t#v}; is_semver "$v" || continue; ver_gt "$v" "$max" && max="$v"; done; if ! $resume; then ver_gt "$new" "$max" || die "target $new <= highest $max"; fi
if ! $resume; then [[ -z "$(git show-ref --tags -q --verify refs/tags/v$new && echo x)" ]] || die "local v$new exists"; [[ -z "$(git ls-remote --tags --refs origin v$new | cat)" ]] || die "remote v$new exists"; fi
tag="v$new"; if $dry; then echo "DRY RUN: would bump to $new and push $tag atomically"; exit 0; fi
if ! $resume; then
  bakdir=$(mktemp -d -t grindvakt-release-XXXXXX); trap 'rm -rf "$bakdir"' EXIT
  cp -f Cargo.toml "$bakdir/Cargo.toml.bak"; test -f Cargo.lock && cp -f Cargo.lock "$bakdir/Cargo.lock.bak" || true
  idx_toml=$(git ls-files -s -- Cargo.toml || true); idx_lock=$(git ls-files -s -- Cargo.lock || true)
  set_ver "$new"
  # Assert manifest==lock root version then update lock root package version
  if ! python3 - <<'PY'
import sys,re
man=re.search(r'^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"', open('Cargo.toml','r',encoding='utf-8').read(), re.M).group(1)
lock=open('Cargo.lock','r',encoding='utf-8').read()
m=re.search(r'(?ms)^\[\[package\]\]\s*name = "grindvakt"\s*version = "([0-9]+\.[0-9]+\.[0-9]+)"', lock)
if not m: sys.exit(3)
open('Cargo.lock','w',encoding='utf-8').write(re.sub(r'(?ms)^(\[\[package\]\]\s*name = "grindvakt"\s*version = ")([0-9]+\.[0-9]+\.[0-9]+)(")', r'\g<1>'+man+r'\3', lock))
PY
  then
    cp -f "$bakdir/Cargo.toml.bak" Cargo.toml; if test -f "$bakdir/Cargo.lock.bak"; then cp -f "$bakdir/Cargo.lock.bak" Cargo.lock; else rm -f Cargo.lock; fi
    if [[ -n "$idx_toml" ]]; then mode=$(echo "$idx_toml"|awk '{print $1}'); sha=$(echo "$idx_toml"|awk '{print $2}'); git update-index --cacheinfo $mode $sha Cargo.toml; else git restore --staged -- Cargo.toml || true; fi
    if [[ -n "$idx_lock" ]]; then mode=$(echo "$idx_lock"|awk '{print $1}'); sha=$(echo "$idx_lock"|awk '{print $2}'); git update-index --cacheinfo $mode $sha Cargo.lock 2>/dev/null || true; else git restore --staged -- Cargo.lock 2>/dev/null || true; fi
    die "lock root update failed"
  fi
  set +e
  cargo fmt --all --check && cargo clippy --workspace --all-targets --all-features --locked -- -D warnings && cargo test --workspace --all-features --locked && cargo build --release --locked
  rc=$?; set -e
  if [[ $rc -ne 0 ]]; then cp -f "$bakdir/Cargo.toml.bak" Cargo.toml; if test -f "$bakdir/Cargo.lock.bak"; then cp -f "$bakdir/Cargo.lock.bak" Cargo.lock; else rm -f Cargo.lock; fi; if [[ -n "$idx_toml" ]]; then mode=$(echo "$idx_toml"|awk '{print $1}'); sha=$(echo "$idx_toml"|awk '{print $2}'); git update-index --cacheinfo $mode $sha Cargo.toml; else git restore --staged -- Cargo.toml || true; fi; if [[ -n "$idx_lock" ]]; then mode=$(echo "$idx_lock"|awk '{print $1}'); sha=$(echo "$idx_lock"|awk '{print $2}'); git update-index --cacheinfo $mode $sha Cargo.lock 2>/dev/null || true; else git restore --staged -- Cargo.lock 2>/dev/null || true; fi; die "gates failed"; fi
  set +e
  git add Cargo.toml Cargo.lock
  add_rc=$?
  set -e
  if [[ $add_rc -ne 0 ]]; then
    # Roll back to pre-add state
    cp -f "$bakdir/Cargo.toml.bak" Cargo.toml; if test -f "$bakdir/Cargo.lock.bak"; then cp -f "$bakdir/Cargo.lock.bak" Cargo.lock; else rm -f Cargo.lock; fi
    if [[ -n "$idx_toml" ]]; then mode=$(echo "$idx_toml"|awk '{print $1}'); sha=$(echo "$idx_toml"|awk '{print $2}'); git update-index --cacheinfo $mode $sha Cargo.toml; else git restore --staged -- Cargo.toml || true; fi
    if [[ -n "$idx_lock" ]]; then mode=$(echo "$idx_lock"|awk '{print $1}'); sha=$(echo "$idx_lock"|awk '{print $2}'); git update-index --cacheinfo $mode $sha Cargo.lock 2>/dev/null || true; else git restore --staged -- Cargo.lock 2>/dev/null || true; fi
    die "git add failed"
  fi
  set +e
  git commit -m "release: v$new"
  commit_rc=$?
  set -e
  if [[ $commit_rc -ne 0 ]]; then
    # Roll back staged/versioned files to pre-commit state
    cp -f "$bakdir/Cargo.toml.bak" Cargo.toml; if test -f "$bakdir/Cargo.lock.bak"; then cp -f "$bakdir/Cargo.lock.bak" Cargo.lock; else rm -f Cargo.lock; fi
    if [[ -n "$idx_toml" ]]; then mode=$(echo "$idx_toml"|awk '{print $1}'); sha=$(echo "$idx_toml"|awk '{print $2}'); git update-index --cacheinfo $mode $sha Cargo.toml; else git restore --staged -- Cargo.toml || true; fi
    if [[ -n "$idx_lock" ]]; then mode=$(echo "$idx_lock"|awk '{print $1}'); sha=$(echo "$idx_lock"|awk '{print $2}'); git update-index --cacheinfo $mode $sha Cargo.lock 2>/dev/null || true; else git restore --staged -- Cargo.lock 2>/dev/null || true; fi
    die "commit failed (pre-commit hook or other error)"
  fi
  set +e
  git tag -a "$tag" -m "$tag"
  tag_rc=$?
  set -e
  if [[ $tag_rc -ne 0 ]]; then
    die "tag creation failed. You can resume safely with: scripts/release/release.sh --resume"
  fi
fi
set +e; out=$(git push --atomic origin HEAD:refs/heads/main "refs/tags/$tag" 2>&1); rc=$?; set -e; if [[ $rc -ne 0 ]]; then if echo "$out" | grep -qi -e "unknown option.*atomic" -e "does not support --atomic push"; then die "remote does not support --atomic; refusing"; fi; echo "$out" >&2; die "atomic push failed"; fi
# Post-verify
r_main=$(git ls-remote --heads origin refs/heads/main | awk '{print $1}'); r_tag=$(git ls-remote --tags --refs origin "$tag" | awk '{print $1}'); l_head=$(git rev-parse HEAD); l_tag=$(git rev-parse "$tag"); [[ "$r_main" == "$l_head" ]] || die "post-push branch mismatch"; [[ "$r_tag" == "$l_tag" ]] || die "post-push tag mismatch"
info "done: $tag"
