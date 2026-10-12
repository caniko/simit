#!/bin/sh
# Native tests pass the verified descriptor directly to this immutable consumer.
set -eu
cat > "$DEPENDENCY_IMPORTED"
printf '%s\n' 'imported validated bytes'
printf '%s\n' 'retained import diagnostic' >&2
