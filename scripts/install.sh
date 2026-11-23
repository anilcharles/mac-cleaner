#!/bin/sh
# Builds the app, quits any running copy, replaces /Applications/Mac Cleaner.app and opens it.
set -e
cd "$(dirname "$0")/.."

./scripts/bundle.sh

APP="target/release/Mac Cleaner.app"
DEST="/Applications/Mac Cleaner.app"
RUNNING="Mac Cleaner.app/Contents/MacOS/mac-cleaner"

# Quit politely first, then make sure every copy is gone.
if pgrep -f "$RUNNING" >/dev/null; then
  echo "Quitting the running Mac Cleaner…"
  osascript -e 'quit app id "com.maccleaner.desktop"' >/dev/null 2>&1 || true
  i=0
  while pgrep -f "$RUNNING" >/dev/null && [ $i -lt 20 ]; do
    sleep 0.25
    i=$((i + 1))
  done
  pkill -f "$RUNNING" 2>/dev/null || true
  sleep 0.5
fi

# Copy next to the old app first, then swap, so a failed copy never leaves you without one.
rm -rf "/Applications/Mac Cleaner.app.new"
ditto "$APP" "/Applications/Mac Cleaner.app.new"
rm -rf "/Applications/Mac Cleaner.app"
mv "/Applications/Mac Cleaner.app.new" "$DEST"
touch "$DEST"   # nudges Finder and the Dock to pick up a changed icon

open "$DEST"
echo "Installed and opened $DEST"
