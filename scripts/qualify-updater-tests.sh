#!/usr/bin/env bash
# Run both hosted updater modes before broad Nix checks can hide a failure.
set -euo pipefail

bash "$(dirname "$0")/qualify-updater-sandbox-host.sh"

status=0
for features in --all-features --no-default-features; do
  printf 'BEGIN updater %s\n' "$features"
  if cargo test --locked "$features" --lib commands::projects_update::; then
    printf 'PASS updater %s\n' "$features"
  else
    status=1
  fi
done
exit "$status"
