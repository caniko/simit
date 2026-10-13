#!/bin/sh
# A descendant effect must never survive cancellation of its owning worker.
set -eu
printf started > "$GROUP_STARTED"
(sleep 2; printf escaped > "$GROUP_LATE_EFFECT") &
wait
