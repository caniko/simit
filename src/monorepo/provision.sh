#!/usr/bin/env bash
# Bound provisioning, including exec, until native deadline ownership.
trap - EXIT
ready=$1
remaining=$(($2 - $(date +%s)))
shift 2
cancelled=0
trap 'cancelled=130' INT
trap 'cancelled=143' TERM
trap 'cancelled=129' HUP
if [ "$remaining" -le 0 ]; then
  echo 'Nix provisioning timed out before execution' >&2
  exit 124
fi
deadline=$((SECONDS + remaining))
# Bash launches async jobs in a new process group before child-side exec.
# This keeps even blocked executable lookup outside the supervising process.
set -m
"$@" &
pid=$!
set +m
status=1
gate_pid=
trap '
  if [ "$status" -ne 0 ]; then
    kill -KILL -- "-$pid" 2>/dev/null || true
    # A killed native runner cannot drop its separate gate process group.
    # Its private marker records that group only after a successful spawn.
    if [ -f "$ready" ] && [ ! -f "$ready.diagnostics-complete" ]; then
      read -r gate_pid < "$ready" || true
      if [[ "$gate_pid" =~ ^[1-9][0-9]*$ ]]; then
        kill -KILL -- "-$gate_pid" 2>/dev/null || true
      fi
    fi
  fi
  # Do not block the diagnostic reserve waiting for an uninterruptible child.
  if ! kill -0 "$pid" 2>/dev/null; then
    wait "$pid" 2>/dev/null || true
  fi
' EXIT
while kill -0 "$pid" 2>/dev/null; do
  if [ "$cancelled" -ne 0 ] || { [ ! -f "$ready" ] && [ "$SECONDS" -ge "$deadline" ]; }; then
    if [ "$cancelled" -ne 0 ]; then
      echo 'Nix provisioning cancelled' >&2
      status=$cancelled
    else
      echo 'Nix provisioning timed out' >&2
      status=124
    fi
    kill -TERM -- "-$pid" 2>/dev/null || true
    grace=5
    if [ -f "$ready" ]; then
      # Native owns its isolated gate and bounded cancellation diagnostics.
      grace=90
    fi
    limit=$((SECONDS + grace))
    while kill -0 "$pid" 2>/dev/null && [ "$SECONDS" -lt "$limit" ]; do
      sleep 0.05
    done
    exit "$status"
  fi
  sleep 0.05
done
if wait "$pid"; then
  status=0
else
  status=$?
fi
exit "$status"
