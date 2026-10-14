#!/bin/sh
set -eu
case "$1" in
  -s) printf '%s\n' "$GROUP_OS" ;;
  -m) printf '%s\n' "$GROUP_ARCH" ;;
  *) exit 98 ;;
esac
