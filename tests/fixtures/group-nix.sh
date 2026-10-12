#!/bin/sh
# Immutable process-boundary fixture; never synthesize executable inodes in tests.
set -eu
printf '%s\n' "$*" >> "$GROUP_CALLS"
if [ "${GROUP_CASE:-}" = bootstrap ]; then
    test -z "${BASH_ENV:-}"
    test -z "${ENV:-}"
    test "$NIX_CONFIG" = 'experimental-features = nix-command flakes'
    test "$NIX_USER_CONF_FILES" = /dev/null
    test -z "${TEST_BINDING:-}"
    printf checked > "$GROUP_BOOTSTRAP_OBSERVATION"
    printf '%s\n' not-a-store-path
    exit 0
fi
case "$1" in
  eval)
    case "${GROUP_CASE:-success}" in
      missing) printf '%s\n' '{"client":"/nix/store/client.drv","recovery":"/nix/store/recovery.drv","new-check":"/nix/store/new.drv"}' ;;
      empty) printf '%s\n' '{}' ;;
      *) printf '%s\n' '{"client":"/nix/store/client.drv","recovery":"/nix/store/recovery.drv"}' ;;
    esac
    ;;
  build)
    printf '%s\n' 'retained builder diagnostic' >&2
    case "${GROUP_CASE:-success}" in
      fail) case "$*" in *client*) exit 7 ;; esac ;;
      malformed) printf '%s\n' 'not-json'; exit 0 ;;
      fresh-fail) case "$*" in *--rebuild*) exit 9 ;; esac ;;
      spawn-fail) rm "$0"; exit 7 ;;
    esac
    printf '%s\n' '[{"drvPath":"/nix/store/result.drv","outputs":{"out":"/nix/store/result"}}]'
    ;;
  *) exit 99 ;;
esac
