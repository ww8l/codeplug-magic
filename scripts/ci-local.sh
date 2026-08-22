#!/usr/bin/env bash
#
# Local equivalent of .github/workflows/ci.yml, minus the OS matrix — it runs
# whatever platform you're sitting at. Wired to the pre-push hook so breakage is
# caught before it costs Actions minutes; CI itself now only runs on PRs.
#
# Keep the step list below in sync with ci.yml.
#
# Usage: npm run ci   (or ./scripts/ci-local.sh)

set -euo pipefail

cd "$(dirname "$0")/.."

step() { printf '\n\033[1;34m==>\033[0m \033[1m%s\033[0m\n' "$1"; }

start=$(date +%s)

step "Build frontend (tsc + vite)"
npm run build

step "cargo check --all-targets"
(cd src-tauri && cargo check --all-targets)

step "cargo test"
(cd src-tauri && cargo test)

# Fatal, matching ci.yml. The pre-existing warnings were cleared in #91, so a
# new one is now a real signal rather than noise on an already-dirty list.
step "cargo clippy --all-targets -- -D warnings"
(cd src-tauri && cargo clippy --all-targets -- -D warnings)

printf '\n\033[1;32m==> CI passed\033[0m (%ss)\n' "$(( $(date +%s) - start ))"
