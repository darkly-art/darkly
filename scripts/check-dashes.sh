#!/usr/bin/env bash
# Fail if any tracked file contains an en dash (U+2013) or em dash (U+2014).
# Use a plain hyphen, comma, colon, or parentheses instead.
set -uo pipefail

cd "$(dirname "$0")/.."

# Built from escapes so this script is not itself a match.
en=$(printf '\u2013')
em=$(printf '\u2014')
# Guard against a printf that does not understand \u and would silently
# turn this into a search for the literal text "\u2013".
if [ ${#en} -ne 1 ] || [ ${#em} -ne 1 ]; then
  echo "check-dashes: printf lacks \\u escape support; cannot build pattern" >&2
  exit 2
fi

# -I skips binaries; grep exits 1 per file with no match, so test the
# collected output rather than the pipeline's status.
hits=$(git ls-files -z | xargs -0 grep -nI -e "$en" -e "$em" --)

if [ -n "$hits" ]; then
  echo "$hits"
  echo
  echo "check-dashes: en/em dashes found (listed above)."
  exit 1
fi
