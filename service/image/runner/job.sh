#!/bin/bash
# Runs in the runner's job container: one verb on the checkout in /ws.
# Environment: BOX_SUBDIR, BOX_VERB_CMD, BOX_TIMEOUT_SECONDS.
# Its exit status is the verb's. It reports nothing else: the verb runs here as the same user,
# so whatever this script wrote or printed about the job, the job could write or print too. How
# long the container ran, how much memory it used and how it ended are read by the runner on
# the host, from its own clock and from Docker.
set -uo pipefail

readonly KILL_GRACE_SECONDS=30

cd "/ws/$BOX_SUBDIR" || exit 2
echo "[box] running: $BOX_VERB_CMD"
timeout --kill-after="$KILL_GRACE_SECONDS" "$BOX_TIMEOUT_SECONDS" bash -c "$BOX_VERB_CMD"
