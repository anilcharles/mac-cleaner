#!/bin/sh
# Controls the self-hosted GitHub Actions runner that builds releases on this Mac.
# The runner itself lives outside the repo (default ~/actions-runner) because it holds credentials.
#
#   ./scripts/runner.sh status   local service state + what GitHub sees
#   ./scripts/runner.sh start    start the background service
#   ./scripts/runner.sh stop     stop it (release builds wait until it is started again)
#   ./scripts/runner.sh restart
#   ./scripts/runner.sh logs     follow the service log (Ctrl+C to stop)
#
# Set RUNNER_DIR to use a runner installed somewhere else.
set -e
cd "$(dirname "$0")/.."

RUNNER_DIR="${RUNNER_DIR:-$HOME/actions-runner}"
REPO="anilcharles/mac-cleaner"

if [ ! -x "$RUNNER_DIR/svc.sh" ]; then
  echo "No runner found in $RUNNER_DIR (set RUNNER_DIR if it is elsewhere)." >&2
  exit 1
fi

svc() { (cd "$RUNNER_DIR" && ./svc.sh "$@"); }

github_status() {
  if command -v gh >/dev/null 2>&1; then
    echo "GitHub sees:"
    gh api "repos/$REPO/actions/runners" \
      -q '.runners[] | "  \(.name): \(.status)\(if .busy then " (building)" else "" end)"' 2>/dev/null \
      || echo "  (could not reach GitHub; is gh logged in?)"
  fi
}

case "$1" in
  status)
    if svc status 2>/dev/null | grep -q "^Started:"; then
      echo "Local service: running"
    else
      echo "Local service: stopped"
    fi
    github_status
    ;;
  start)   svc start ;;
  stop)    svc stop ;;
  restart) svc stop; svc start ;;
  logs)
    LOG_DIR=$(ls -d "$HOME/Library/Logs/actions.runner."* 2>/dev/null | head -1)
    if [ -z "$LOG_DIR" ]; then
      echo "No service logs found in ~/Library/Logs." >&2
      exit 1
    fi
    tail -n 50 -f "$LOG_DIR"/*.log
    ;;
  *)
    echo "Usage: $0 {status|start|stop|restart|logs}" >&2
    exit 2
    ;;
esac
