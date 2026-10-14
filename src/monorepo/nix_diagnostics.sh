printf '=== Nix failure diagnostics (check start @%s) ===\n' "$1" >&2
if [ "$(uname -s)" != Linux ]; then
  printf '%s\n' 'Linux kernel/systemd diagnostics unavailable on this platform' >&2
  exit 0
fi
for path in /proc/meminfo /proc/pressure/memory; do
  printf '\n--- %s ---\n' "$path" >&2
  cat -- "$path" >&2 || true
done
capture() {
  printf '\n--- %s ---\n' "$1" >&2
  shift
  if timeout --kill-after=5s 15s sudo -n "$@" >&2; then
    printf 'status: 0\n' >&2
  else
    printf 'status: %s\n' "$?" >&2
  fi
}
capture 'Nix daemon status' systemctl show nix-daemon.service --property=ActiveState,SubState,Result,ExecMainCode,ExecMainStatus,MemoryCurrent,MemoryPeak,OOMPolicy
capture 'Nix daemon journal' journalctl --unit=nix-daemon.service --since "@$1" --lines=150 --no-pager --output=short-iso
capture 'Kernel journal' journalctl --dmesg --since "@$1" --lines=150 --no-pager --output=short-iso
