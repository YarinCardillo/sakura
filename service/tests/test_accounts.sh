#!/bin/bash
# box accounts --json: loggedIn is what `claude auth status --json` reports inside a confined
# container with the account volume; deadline per account, accounts checked concurrently.
. "$(dirname "$0")/lib.sh"
setup_sandbox
export FAKE_ACCOUNTS="personal work"

echo " the fields and the container"
OUT="$("$BOX" accounts --json 2>/dev/null)"
assert_eq "valid JSON array of two" "2" "$(jq 'length' <<<"$OUT")"
assert_eq "exactly the documented fields" '["authMethod","checkedAt","loggedIn","name","where"]' "$(jq -c '.[0] | keys' <<<"$OUT")"
assert_eq "name" "personal" "$(jq -r '.[0].name' <<<"$OUT")"
assert_eq "logged in as reported" "true" "$(jq -r '.[0].loggedIn' <<<"$OUT")"
assert_eq "auth method as reported" "claude.ai" "$(jq -r '.[0].authMethod' <<<"$OUT")"
assert_eq "checkedAt is UTC" "1" "$(jq -r '.[0].checkedAt | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$") | if . then 1 else 0 end' <<<"$OUT")"
CALL="$(grep -e 'acctcheck-personal ' "$FAKE_DOCKER_LOG" | head -1)"
assert_contains "runs claude auth status --json" "claude auth status --json" "$CALL"
assert_contains "account volume as the config directory" "-v agentbox-acct-personal:/acct" "$CALL"
assert_contains "confined network" "--network agentbox-net" "$CALL"
assert_contains "no capabilities" "--cap-drop ALL" "$CALL"
assert_contains "read-only root" "--read-only" "$CALL"

echo " a credentials file is no longer enough"
export FAKE_ACCOUNT_STATUS_work='{"loggedIn": false}'
OUT="$("$BOX" accounts --json 2>/dev/null)"
assert_eq "logged out as reported" "false" "$(jq -r '.[] | select(.name == "work") | .loggedIn' <<<"$OUT")"
assert_eq "no auth method" "null" "$(jq -c '.[] | select(.name == "work") | .authMethod' <<<"$OUT")"
export FAKE_ACCOUNT_STATUS_work='not json at all'
OUT="$("$BOX" accounts --json 2>/dev/null)"
assert_eq "unreadable status counts as logged out" "false" "$(jq -r '.[] | select(.name == "work") | .loggedIn' <<<"$OUT")"

echo " nothing else the status says is printed"
export FAKE_ACCOUNT_STATUS_work='{"loggedIn": true, "authMethod": "claude.ai", "email": "secret@example.com", "apiKey": "sk-secret-value"}'
OUT="$("$BOX" accounts --json 2>&1)"
assert_eq "no secret in the output" "0" "$(grep -c -e secret <<<"$OUT")"
export FAKE_ACCOUNT_STATUS_work='{"loggedIn": true, "authMethod": "x; rm -rf / $(evil)"}'
assert_eq "an odd auth method is null" "null" "$("$BOX" accounts --json 2>/dev/null | jq -c '.[] | select(.name == "work") | .authMethod')"
unset FAKE_ACCOUNT_STATUS_work

echo " accounts are checked concurrently"
export FAKE_ACCOUNTS="a b c" FAKE_ACCOUNT_DELAY=2
START="$(date +%s)"
OUT="$("$BOX" accounts --json 2>/dev/null)"
assert_eq "three accounts" "3" "$(jq length <<<"$OUT")"
assert_eq "order is the volume order" '["a","b","c"]' "$(jq -c '[.[].name]' <<<"$OUT")"
assert_eq "faster than one after the other" "1" "$([ $(( $(date +%s) - START )) -lt 5 ] && echo 1 || echo 0)"

echo " the deadline is per account"
export FAKE_ACCOUNT_DELAY=30 BOX_ACCOUNT_CHECK_SECONDS=2
START="$(date +%s)"
OUT="$("$BOX" accounts --json 2>/dev/null)"
assert_eq "it did not wait for the slow checks" "1" "$([ $(( $(date +%s) - START )) -lt 10 ] && echo 1 || echo 0)"
assert_eq "a check that timed out is logged out" "false" "$(jq -r '.[0].loggedIn' <<<"$OUT")"
assert_contains "the check container is removed by name" "rm -f agentbox-acctcheck-" "$(cat "$FAKE_DOCKER_LOG")"
assert_eq "no accounts, empty array" "[]" "$(FAKE_ACCOUNTS="" "$BOX" accounts --json)"
finish
