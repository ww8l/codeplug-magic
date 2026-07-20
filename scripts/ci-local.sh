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

# Non-fatal, matching ci.yml: there are pre-existing clippy warnings. Once those
# are cleaned up, tighten both this and ci.yml to `-- -D warnings`.
step "cargo clippy --all-targets"
(cd src-tauri && cargo clippy --all-targets)

printf '\n\033[1;32m==> CI passed\033[0m (%ss)\n' "$(( $(date +%s) - start ))"
