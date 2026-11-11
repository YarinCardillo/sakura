#!/bin/bash
# Runs in the runner's install container: installs dependencies from package.json and the
# lockfile only (copied to /ws from the read-only /stage), with install scripts disabled.
# The sources of the commit are not visible here.
# Environment: BOX_INSTALL, BOX_INSTALL_TIMEOUT_SECONDS, BOX_REBUILD (optional command).
set -euo pipefail

readonly KILL_GRACE_SECONDS=30

cp /stage/package.json /stage/package-lock.json /ws/
cd /ws
export npm_config_ignore_scripts=true
timeout --kill-after="$KILL_GRACE_SECONDS" "$BOX_INSTALL_TIMEOUT_SECONDS" bash -c "$BOX_INSTALL"
if [ -n "${BOX_REBUILD:-}" ]; then
  echo "[box] rebuilding packages that need their own scripts"
  unset npm_config_ignore_scripts
  timeout --kill-after="$KILL_GRACE_SECONDS" "$BOX_INSTALL_TIMEOUT_SECONDS" bash -c "$BOX_REBUILD"
fi
echo "[box] dependencies installed"
