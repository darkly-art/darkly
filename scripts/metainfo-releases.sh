#!/usr/bin/env bash
# Fill the AppStream <releases> block of a metainfo file from git tags.
#
#   scripts/metainfo-releases.sh packaging/art.darkly.Darkly.metainfo.xml > out.xml
#
# The release record is the annotated vX.Y.Z tag (docs/versioning.md): its name
# is the version, its tagger date the release date, and each non-blank line of
# its body one change. Only tags reachable from HEAD count, so a checkout of an
# older release never lists a later one. A lightweight tag, or an annotated tag
# with an empty body, renders with no <description>, which AppStream accepts.
#
# The checked-in metainfo carries a copy of the block, refreshed by
# scripts/release.sh after each tag and committed. This script replaces that
# block, or inserts one before </component> if the file has none. With no
# repository or no tags (a tarball, a shallow clone) it leaves an existing block
# as committed rather than emptying it, and inserts an empty one otherwise.
#
# Offline, git and POSIX tools only: it runs unchanged in CI, locally, and in
# the Flathub build sandbox, which compiles no native Rust.
#
# The XML escape below is the shell twin of `product::xml_escape`
# (crates/darkly/src/product.rs); change one, change the other.
set -euo pipefail

# The one home of this URL is `repository` in the root Cargo.toml. It is a
# literal here because the Flathub sandbox has no `origin` to derive it from.
repo_url="https://github.com/darkly-art/darkly"

metainfo="${1:?usage: $0 <metainfo.xml>}"

die() { echo "metainfo-releases: $metainfo $*" >&2; exit 1; }

own_line() { grep -cxE "[[:space:]]*$1[[:space:]]*" "$metainfo" || true; }

opens=$(own_line '<releases>')
closes=$(own_line '</releases>')
if [ "$(grep -c '<releases' "$metainfo" || true)" -ne "$opens" ] \
   || [ "$(grep -c '</releases>' "$metainfo" || true)" -ne "$closes" ] \
   || [ "$opens" -gt 1 ] || [ "$opens" -ne "$closes" ]; then
  die "must have at most one <releases>...</releases>, each tag on its own line"
fi
if [ "$opens" -eq 0 ] && { [ "$(grep -c '</component>' "$metainfo" || true)" -ne 1 ] \
   || [ "$(own_line '</component>')" -ne 1 ]; }; then
  die "must contain exactly one </component>, on its own line"
fi

xml_escape() {
  sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g' \
      -e 's/"/\&quot;/g' -e "s/'/\&apos;/g"
}

tags=""
if git rev-parse --git-dir >/dev/null 2>&1; then
  tags=$(git for-each-ref --merged HEAD --sort=-v:refname \
           --format='%(refname:strip=2) %(objecttype) %(creatordate:short)' 'refs/tags/v*' \
         | { grep -E '^v[0-9]+\.[0-9]+\.[0-9]+ ' || true; })
fi

if [ -z "$tags" ] && [ "$opens" -eq 1 ]; then
  cat "$metainfo"
  exit 0
fi

render_releases() {
  echo "  <releases>"
  [ -n "$tags" ] && printf '%s\n' "$tags" | while read -r tag type date; do
    echo "    <release version=\"${tag#v}\" date=\"$date\">"
    echo "      <url type=\"details\">$repo_url/releases/tag/$tag</url>"
    body=""
    if [ "$type" = tag ]; then
      body=$(git for-each-ref --format='%(contents:body)' "refs/tags/$tag" \
               | sed '/^[[:space:]]*$/d')
    fi
    if [ -n "$body" ]; then
      echo "      <description>"
      echo "        <ul>"
      printf '%s\n' "$body" | xml_escape | sed 's|.*|          <li>&</li>|'
      echo "        </ul>"
      echo "      </description>"
    fi
    echo "    </release>"
  done
  echo "  </releases>"
}

block=$(render_releases)
in_block=""
while IFS= read -r line || [ -n "$line" ]; do
  if [ "$opens" -eq 1 ]; then
    if [[ "$line" =~ ^[[:space:]]*\<releases\>[[:space:]]*$ ]]; then
      printf '%s\n' "$block"; in_block=1; continue
    fi
    if [ -n "$in_block" ]; then
      [[ "$line" =~ ^[[:space:]]*\</releases\>[[:space:]]*$ ]] && in_block=""
      continue
    fi
  elif [[ "$line" =~ ^[[:space:]]*\</component\>[[:space:]]*$ ]]; then
    printf '%s\n\n' "$block"
  fi
  printf '%s\n' "$line"
done < "$metainfo"
