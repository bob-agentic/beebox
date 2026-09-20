#!/usr/bin/env bash
# One command: build everything, quit the running BeeBox, open the fresh one.
#
#   ./mac/scripts/run.sh
#
# Always does the full build (ui → core → swift → bundle → sign): the front
# end is embedded into the Rust binary at compile time, so partial builds
# produce an app running yesterday's UI — not worth a flag.
set -euo pipefail
cd "$(dirname "$0")/../.."

step() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }

step "build (ui → core → swift → bundle → sign)"
./mac/scripts/build-app.sh

step "quit the running BeeBox (if any)"
# Graceful first so cleanup traps run; the app owns its daemon
# (--exit-with-parent), so quitting the app takes the old daemon with it.
osascript -e 'tell application "BeeBox" to quit' >/dev/null 2>&1 || true
for _ in $(seq 1 20); do
  pgrep -x BeeBox >/dev/null || break
  sleep 0.2
done
pgrep -x BeeBox >/dev/null && { pkill -x BeeBox || true; sleep 0.5; }

step "launch"
open mac/build/BeeBox.app
echo "done — new BeeBox is up"
