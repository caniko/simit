#!/bin/sh
[ "$GH_TOKEN" = fixture-report-token ] && [ "$PLAN_DIGEST" = fixture-plan-digest ] || exit 1
touch report-posted || exit
printf '%s\n' '{"posted":true}'
