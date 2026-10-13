#!/bin/sh
printf '%s\n' "$POST_REQUESTED" >> jq-invoked
[ "$POST_REQUESTED" = true ]
