#!/bin/sh
# Runs `chaperone hook <event>` in a lane. Silent and exit 0 otherwise: no
# lane, no binary, or one too old to know `hook` must never break a session.
[ -n "$CHAPERONE_WORK" ] || exit 0
out=$(chaperone hook "$1" 2>/dev/null) && [ -n "$out" ] && printf '%s\n' "$out"
exit 0
