#!/usr/bin/env bash
# Everything that can be checked without a person watching.
set -euo pipefail
cd "$(dirname "$0")/../.."

step() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }

step "unit tests"
pnpm --dir ui test

step "daemon tests"
cargo test --manifest-path core/Cargo.toml

step "menu ↔ page contract"
swift run --package-path mac beebox-contract

step "browser path"
pnpm --dir ui test:e2e

# The one the browser suite cannot reach: a real menu bar, in the real app.
# Every shortcut bug so far has lived precisely here.
step "desktop path"
pkill -f beebox-core 2>/dev/null || true
sleep 1
mac/build/BeeBox.app/Contents/MacOS/BeeBox --self-test

printf '\n\033[1mall green\033[0m\n'
