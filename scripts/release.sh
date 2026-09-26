#!/usr/bin/env bash
# Cut a release: tag vX.Y.Z on dev with its changes as the tag body, push it,
# and merge the dev -> master release PR.
#
#   scripts/release.sh 0.9.0          # shows the changes, asks, then tags
#   scripts/release.sh 0.9.0 --yes    # no prompt
#
# The annotated tag is the release record (docs/versioning.md). Every check
# runs before the tag push, which is the irreversible step: it fires
# publish.yml (crates.io) and docs-artifact.yml. That includes CI: the release
# PR's checks must have passed on the exact commit being tagged. Afterwards the
# PR is merged and the checked-in metainfo's <releases> block is refreshed from
# the tags; this script never commits, so it ends by printing the commit to make.
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
[ -z "$(git status --porcelain --untracked-files=no)" ] || die "tracked files have uncommitted changes"
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
        --json number,title,author,mergeCommit,url \
        --jq '.[] | [(.mergeCommit.oid // ""), (.author.is_bot | tostring), (.number | tostring), .url, .title] | @tsv')
commits=$(git rev-list --reverse "$range")

# gh returns newest first; if the list is full and its oldest entry is still
# inside the range, older PRs in the range were cut off.
if [ "$(printf '%s\n' "$prs" | grep -c .)" -ge "$limit" ]; then
  oldest=$(printf '%s\n' "$prs" | tail -1 | cut -f1)
  printf '%s\n' "$commits" | grep -qxF "$oldest" \
    && die "more than $limit merged PRs; the range may be truncated"
fi

# One line per PR in the range: `Title (#N)` for the tag body, and the PR's
# URL for the release PR body (an unordered list of links, numerically sorted,
# which is how every release PR on this repository is written).
picked=$(awk -F'\t' '
  NR == FNR { if ($1 != "" && $2 == "false") pr[$1] = $3 "\t" $4 "\t" $5; next }
  $0 in pr  { print pr[$0] }
' <(printf '%s\n' "$prs") <(printf '%s\n' "$commits"))
notes=$(printf '%s\n' "$picked" | awk -F'\t' 'NF { print $3 " (#" $1 ")" }')
links=$(printf '%s\n' "$picked" | awk -F'\t' 'NF { print $1 "\t- " $2 }' | sort -n | cut -f2-)

[ -n "$notes" ] || die "no merged pull requests between ${prev:-the first commit} and HEAD"
if bad=$(printf '%s\n' "$notes" | grep -F '://'); then
  die "the store listing cannot carry a URL; retitle on GitHub and re-run:
$bad"
fi

# The release PR: its CI is the gate on what gets tagged. With none open, open
# one and stop; its checks run on this commit and the next run can proceed.
title="Dev -> Master $version"
pr=$(gh pr list --base master --head dev --state open --json number --jq '.[0].number // empty')
if [ -z "$pr" ]; then
  printf '%s\n' "$links" | gh pr create --base master --head dev --title "$title" --body-file -
  die "opened the release PR; run this again once its checks pass"
fi
[ "$(gh pr view "$pr" --json headRefOid --jq .headRefOid)" = "$(git rev-parse HEAD)" ] \
  || die "release PR #$pr is not at HEAD yet; wait for GitHub to catch up"
if ! pending=$(gh pr checks "$pr" --json name,bucket \
                 --jq '.[] | select(.bucket != "pass" and .bucket != "skipping") | "\(.name): \(.bucket)"'); then
  die "could not read the checks of release PR #$pr"
fi
[ -z "$pending" ] || die "release PR #$pr is not green:
$pending"

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

# The release PR body is the list of PR links, and the PR is merged as a merge
# commit, so master contains the tag. A failure here does not
# undo the release; it is reported and the PR can be merged by hand.
printf '%s\n' "$links" | gh pr edit "$pr" --title "$title" --body-file -
gh pr merge "$pr" --merge || echo "release: could not merge release PR #$pr; merge it by hand" >&2

# The checked-in copy of the release history, refreshed now that the tag exists.
refreshed=$(mktemp)
"$here/metainfo-releases.sh" "$metainfo" > "$refreshed"
cat "$refreshed" > "$metainfo"
rm -f "$refreshed"

cat <<DONE

Tagged and pushed $tag, merged the release PR. The last step
(docs/versioning.md, "Cutting a release"):

  git commit $metainfo -m "Release history for $tag"
  git push origin dev
DONE
