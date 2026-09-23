#!/usr/bin/env bash
# Builds BeeBox.app from a clean checkout. No Xcode: SwiftPM produces the
# executable and the bundle is assembled here.
#
#   ./mac/scripts/build-app.sh        the app you ship
#   ./mac/scripts/build-app.sh dev    "BeeBox Dev", which runs beside it
#
# The dev build has its own bundle id, so it launches alongside an installed
# BeeBox rather than bringing that one forward, and its own daemon home
# (~/.beebox-dev), so the two never share workspaces or terminals.
set -euo pipefail

cd "$(dirname "$0")/../.."
ROOT=$PWD
case "${1:-}" in
  "") NAME=BeeBox; ID=dev.beebox.app; HOME_DIR=.beebox ;;
  dev) NAME="BeeBox Dev"; ID=dev.beebox.app.dev; HOME_DIR=.beebox-dev ;;
  *) echo "usage: $0 [dev]" >&2; exit 64 ;;
esac
APP="mac/build/$NAME.app"

step() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }

# The daemon embeds ui/dist at compile time, so the front end must be built
# first. Reversed, you get a daemon serving a stale interface and no error to
# tell you why.
step "building the interface"
pnpm --dir ui build

step "building the daemon"
cargo build --release --manifest-path core/Cargo.toml

step "building the shell"
swift build -c release --package-path mac

step "assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp mac/.build/release/BeeBoxShell "$APP/Contents/MacOS/BeeBox"
cp core/target/release/beebox-core "$APP/Contents/MacOS/beebox-core"
sed -e "s/__VERSION__/$(grep -m1 '^version' core/Cargo.toml | cut -d'"' -f2)/" \
    -e "s/__NAME__/$NAME/" -e "s/__ID__/$ID/" -e "s/__HOME__/$HOME_DIR/" \
    mac/Resources/Info.plist.in > "$APP/Contents/Info.plist"

step "drawing the icon"
mac/scripts/make-icns.sh "$APP/Contents/Resources/AppIcon.icns"

step "signing"
# Ad-hoc: enough to run locally. A distributed build needs a real identity.
codesign --force --deep --sign - "$APP"

printf '\n\033[1m%s\033[0m  (%s)\n' "$ROOT/$APP" "$(du -sh "$APP" | cut -f1)"
