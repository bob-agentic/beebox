#!/usr/bin/env bash
# One command: build BeeBox Dev, quit the previous one, open the fresh one.
#
#   ./mac/scripts/run.sh
#
# Always the dev build, which runs beside an installed BeeBox with its own
# daemon and workspaces — so trying a change never takes down the terminals
# you are working in. Only BeeBox Dev is quit here, by bundle id; matching by
# name would catch the installed app too.
#
# Always does the full build (ui → core → swift → bundle → sign): the front
# end is embedded into the Rust binary at compile time, so partial builds
# produce an app running yesterday's UI — not worth a flag.
set -euo pipefail
cd "$(dirname "$0")/../.."

ID=dev.beebox.app.dev
# The shell process of this build and no other: the installed app's path has
# no " Dev", and its daemon is `beebox-core`, not `BeeBox`.
PROC="BeeBox Dev.app/Contents/MacOS/BeeBox"

step() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }

step "build BeeBox Dev (ui → core → swift → bundle → sign)"
./mac/scripts/build-app.sh dev

step "quit the running BeeBox Dev (if any)"
# Graceful, so cleanup runs; the app owns its daemon (--exit-with-parent), so
# quitting it takes that daemon along. Only when running: telling an app that
# is not running to quit can launch it first.
if pgrep -f "$PROC" >/dev/null; then
  osascript -e "tell application id \"$ID\" to quit" >/dev/null 2>&1 || true
  for _ in $(seq 1 20); do
    pgrep -f "$PROC" >/dev/null || break
    sleep 0.2
  done
  pkill -f "$PROC" 2>/dev/null && sleep 0.5 || true
fi

step "launch"
open "mac/build/BeeBox Dev.app"
echo "done — BeeBox Dev is up"
