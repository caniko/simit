#!/bin/sh
# Executed via per-test symlinks; no canonical Nix store is mutated.
set -eu
root=$(dirname "$0")
printf '%s\n' "$*" >> "$root/effects"
case "$1" in
  --import)
    cat > "$root/imported"
    if test -f "$root/import-fail"; then exit 7; fi
    printf '%s\n' imported
    printf '%s\n' 'retained import diagnostic' >&2
    ;;
  --verify-path)
    if test -f "$root/verify-fail"; then exit 8; fi
    ;;
  path-info)
    if test -f "$root/roots-fail"; then exit 9; fi
    ;;
  *) exit 99 ;;
esac
