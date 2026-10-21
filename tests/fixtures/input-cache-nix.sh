#!/bin/sh
# Immutable process-boundary fixture for `nix` and `nix-store` in input-cache steps.
set -eu
printf '%s %s\n' "${0##*/}" "$*" >> "$CACHE_CALLS"
case "${0##*/}:$*" in
  "nix:path-info --derivation --no-update-lock-file .#checks.x86_64-linux.client .#checks.x86_64-linux.recovery")
    printf '%s\n' /nix/store/00000000000000000000000000000001-client.drv /nix/store/00000000000000000000000000000002-recovery.drv ;;
  "nix:path-info --derivation --no-update-lock-file .#packages.x86_64-linux.kernel")
    printf '%s\n' /nix/store/00000000000000000000000000000003-kernel.drv ;;
  nix-store:--query\ --outputs\ /nix/store/00000000000000000000000000000001-client.drv*)
    printf '%s\n' /nix/store/client-out /nix/store/recovery-out ;;
  "nix-store:--query --outputs /nix/store/00000000000000000000000000000003-kernel.drv")
    printf '%s\n' /nix/store/kernel-out /nix/store/kernel-dev ;;
  nix:path-info\ --store\ file://*\ --recursive\ /nix/store/kernel-out\ /nix/store/kernel-dev)
    printf '%s\n' /nix/store/kernel-out /nix/store/kernel-dev $CACHE_CLOSURE ;;
  nix:copy\ *) : ;;
  *) exit 99 ;;
esac
