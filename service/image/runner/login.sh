#!/bin/bash
# Runs inside the login container (config directory = the account volume).
# Exit codes: 0 logged in, 2 sign-in not completed, 3 finished but not logged in,
# 4 the status could not be read, 5 no connection to the sign-in service.
readonly EXIT_NOT_COMPLETED=2
readonly EXIT_NOT_LOGGED_IN=3
readonly EXIT_STATUS_UNREADABLE=4
readonly EXIT_NO_CONNECTION=5
readonly PROBE_URL="https://api.anthropic.com"
readonly PROBE_SECONDS=10

# Tells a failed sign-in apart from a network that is down.
login_failure_code() {
  if curl -sS -o /dev/null --max-time "$PROBE_SECONDS" "$PROBE_URL" 2>/dev/null; then
    echo "$EXIT_NOT_COMPLETED"
  else
    echo "$EXIT_NO_CONNECTION"
  fi
}

# Succeeds only when `claude auth status --json` says "loggedIn": true.
logged_in() {
  local status code=0
  status="$(claude auth status --json)" || return "$EXIT_STATUS_UNREADABLE"
  printf '%s' "$status" | python3 -c 'import json, sys
try:
    sys.exit(0 if json.load(sys.stdin).get("loggedIn") is True else 1)
except Exception:
    sys.exit(2)' || code=$?
  case "$code" in
    0) return 0 ;;
    1) return "$EXIT_NOT_LOGGED_IN" ;;
    *) return "$EXIT_STATUS_UNREADABLE" ;;
  esac
}

main() {
  if ! claude auth login; then
    exit "$(login_failure_code)"
  fi
  logged_in
  exit $?
}

main "$@"
