#!/bin/sh
# Prints the CHANGELOG.md section for one version, without its heading, for use as release notes.
#   ./scripts/changelog.sh 1.0.0
# Exits 1 if CHANGELOG.md has no "## [<version>]" section, so a release cannot go out without notes.
set -e
cd "$(dirname "$0")/.."
VERSION="${1:?usage: $0 <version>}"

NOTES=$(awk -v head="## [$VERSION]" '
  /^## \[/ { if (found) exit; if (index($0, head) == 1) { found = 1; next } }
  /^\[[^]]+\]: / { next }          # link definitions at the end of the file
  found { print }
' CHANGELOG.md | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}' | sed '/./,$!d')

if [ -z "$NOTES" ]; then
  echo "CHANGELOG.md has no section for $VERSION. Add \"## [$VERSION] - YYYY-MM-DD\" before releasing." >&2
  exit 1
fi
printf '%s\n' "$NOTES"
