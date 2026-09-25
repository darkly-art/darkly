#!/usr/bin/env bash
# Cut a release: tag vX.Y.Z on dev with its changes as the tag body, push it,
# and point the dev -> master release PR at it.
#
#   scripts/release.sh 0.9.0          # shows the changes, asks, then tags
#   scripts/release.sh 0.9.0 --yes    # no prompt
#
# The annotated tag is the release record (docs/versioning.md). Every check
# runs before the tag push, which is the irreversible step: it fires
# publish.yml (crates.io) and docs-artifact.yml. Afterwards the checked-in
# metainfo's <releases> block is refreshed from the tags; this script never
# commits, so it ends by printing the commit to make.
#
# Needs git and an authenticated gh. Writing the tag by hand with `git tag -a`
# is equally valid; this script only saves typing and adds the refusals.
set -euo pipefail

die() { echo "release: $*" >&2; exit 1; }

here=$(cd "$(dirname "$0")" && pwd)
cd "$(git rev-parse --show-toplevel)"
metainfo=packaging/art.darkly.Darkly.metainfo.xml
[ -f "$metainfo" ] || die "no $metainfo in this checkout"

version="${1:-}"
yes=""
[ "${2:-}" = "--yes" ] && yes=1
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "usage: $0 X.Y.Z [--yes]"
tag="v$version"

# Checks. Tags are synced from origin, overwriting local ones that differ:
# GitHub holds the real tags, and the range below must agree with it.
[ "$(git rev-parse --abbrev-ref HEAD)" = dev ] || die "not on dev"
[ -z "$(git status --porcelain)" ] || die "working tree is not clean"
git fetch --quiet origin dev '+refs/tags/*:refs/tags/*'
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/dev)" ] \
  || die "HEAD is not origin/dev; push or pull first"
git rev-parse -q --verify "refs/tags/$tag" >/dev/null && die "$tag already exists"

prev=$(git tag --merged HEAD --list 'v*' --sort=-v:refname \
         | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | head -1 || true)
if [ -n "$prev" ]; then
  printf '%s\n%s\n' "${prev#v}" "$version" | sort -V -C -u \
    || die "$version is not above the last release $prev"
  range="$prev..HEAD"
else
  range="HEAD"
fi

# Changes: merged PRs whose merge commit is in the range, bots excluded, in
# merge order, one `Title (#N)` line each. `mergeCommit` is the commit on the
# base branch for merge, squash and rebase merges alike.
limit=200
prs=$(gh pr list --state merged --limit "$limit" \
        --json number,title,author,mergeCommit \
        --jq '.[] | [(.mergeCommit.oid // ""), (.author.is_bot | tostring), (.number | tostring), .title] | @tsv')
commits=$(git rev-list --reverse "$range")

# gh returns newest first; if the list is full and its oldest entry is still
# inside the range, older PRs in the range were cut off.
if [ "$(printf '%s\n' "$prs" | grep -c .)" -ge "$limit" ]; then
  oldest=$(printf '%s\n' "$prs" | tail -1 | cut -f1)
  printf '%s\n' "$commits" | grep -qxF "$oldest" \
    && die "more than $limit merged PRs; the range may be truncated"
fi

notes=$(awk -F'\t' '
  NR == FNR { if ($1 != "" && $2 == "false") pr[$1] = $4 " (#" $3 ")"; next }
  $0 in pr  { print pr[$0] }
' <(printf '%s\n' "$prs") <(printf '%s\n' "$commits"))

[ -n "$notes" ] || die "no merged pull requests between ${prev:-the first commit} and HEAD"
if bad=$(printf '%s\n' "$notes" | grep -F '://'); then
  die "the store listing cannot carry a URL; retitle on GitHub and re-run:
$bad"
fi

echo "Release $tag, ${prev:-first release}..HEAD ($(git rev-parse --short HEAD)):"
echo
printf '%s\n' "$notes" | sed 's/^/  /'
echo
if [ -z "$yes" ]; then
  read -r -p "Tag and push $tag? This publishes to crates.io. [y/N] " answer
  [ "$answer" = y ] || die "aborted, nothing done"
fi

printf '%s\n\n%s\n' "$tag" "$notes" | git tag -a "$tag" -F -
git push origin "$tag"

# The release PR's body is the tag body verbatim; GitHub links each (#N).
title="Dev -> Master $version"
open=$(gh pr list --base master --head dev --state open --json number --jq '.[0].number // empty')
if [ -n "$open" ]; then
  printf '%s\n' "$notes" | gh pr edit "$open" --title "$title" --body-file -
else
  printf '%s\n' "$notes" | gh pr create --base master --head dev --title "$title" --body-file -
fi

# The checked-in copy of the release history, refreshed now that the tag exists.
refreshed=$(mktemp)
"$here/metainfo-releases.sh" "$metainfo" > "$refreshed"
cat "$refreshed" > "$metainfo"
rm -f "$refreshed"

cat <<DONE

Tagged and pushed $tag. Next (docs/versioning.md, "Cutting a release"):

  git commit $metainfo -m "Release history for $tag"
  git push origin dev
DONE
