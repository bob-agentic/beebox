#!/usr/bin/env bash
# Builds BeeBox.app from a clean checkout. No Xcode: SwiftPM produces the
# executable and the bundle is assembled here.
set -euo pipefail

cd "$(dirname "$0")/../.."
ROOT=$PWD
APP=mac/build/BeeBox.app

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
sed "s/__VERSION__/$(grep -m1 '^version' core/Cargo.toml | cut -d'"' -f2)/" \
    mac/Resources/Info.plist.in > "$APP/Contents/Info.plist"

step "drawing the icon"
mac/scripts/make-icns.sh "$APP/Contents/Resources/AppIcon.icns"

step "signing"
# Ad-hoc: enough to run locally. A distributed build needs a real identity.
codesign --force --deep --sign - "$APP"

printf '\n\033[1m%s\033[0m  (%s)\n' "$ROOT/$APP" "$(du -sh "$APP" | cut -f1)"
