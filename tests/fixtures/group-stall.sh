#!/bin/sh
# A descendant effect must never survive cancellation of its owning worker.
set -eu
exec 3<"$GROUP_INPUT"
(
  cat <&3 >/dev/null
  printf escaped >"$GROUP_LATE_EFFECT"
) &
printf started >"$GROUP_STARTED"
wait
