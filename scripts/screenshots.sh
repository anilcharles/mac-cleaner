#!/bin/sh
# Regenerates docs/screenshots/ for the README from made-up sample data.
# Nothing on this Mac is scanned or cleaned, and saved settings are not changed.
# Keep the Mac awake and leave it alone while this runs (about a minute): the app's window
# comes to the front and takes keyboard focus, so every picture shows an active window.
set -e
cd "$(dirname "$0")/.."
cargo build
OUT="docs/screenshots"
TMP="$(mktemp -d)"
mkdir -p "$OUT"

run() { # theme zoom
  mkdir -p "$TMP/$1-$2"
  MAC_CLEANER_SAMPLE=1 MAC_CLEANER_THEME="$1" MAC_CLEANER_ZOOM="$2" \
    MAC_CLEANER_SNAPSHOT_DIR="$TMP/$1-$2" ./target/debug/mac-cleaner 2>/dev/null
}

run light 1.0
cp "$TMP/light-1.0/1-welcome.png"      "$OUT/welcome.png"
cp "$TMP/light-1.0/2-scanning.png"     "$OUT/scanning.png"
cp "$TMP/light-1.0/3-results.png"      "$OUT/results.png"
cp "$TMP/light-1.0/4-tree.png"         "$OUT/folder-tree.png"
cp "$TMP/light-1.0/4b-large-files.png" "$OUT/large-files.png"
cp "$TMP/light-1.0/4b3-leftovers.png"  "$OUT/app-leftovers.png"
cp "$TMP/light-1.0/4c-search.png"      "$OUT/search.png"
cp "$TMP/light-1.0/4d-row-menu.png"    "$OUT/row-menu.png"
cp "$TMP/light-1.0/5-confirm.png"      "$OUT/confirm.png"
cp "$TMP/light-1.0/7-done.png"         "$OUT/done.png"
cp "$TMP/light-1.0/4g-update.png"      "$OUT/update.png"
cp "$TMP/light-1.0/4f-about-update.png" "$OUT/about-update.png"

run dark 1.0
cp "$TMP/dark-1.0/3-results.png"       "$OUT/results-dark.png"

# The captures are Retina-sized (2 pixels per point). Bring every picture to the same
# 1120 x 740, the window's size in points, to keep the repository small and the README even.
rm -f "$OUT/results-zoom-150.png"
for f in "$OUT"/*.png; do
  sips -z 740 1120 "$f" >/dev/null
done

echo "Wrote $(ls "$OUT" | wc -l | tr -d ' ') screenshots to $OUT"
