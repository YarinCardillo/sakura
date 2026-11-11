#!/bin/bash
# Boxes of one account do not share their configuration directory: each task has its own volume
# for /acct, and only the credential files of the account are mounted into it, as single files.
. "$(dirname "$0")/lib.sh"
setup_sandbox
export FAKE_ACCOUNTS="personal"
"$BOX" new t1 sandbox >/dev/null 2>&1
"$BOX" new t2 sandbox >/dev/null 2>&1

# agent_call <task> <account>: starts a fresh agent and prints its docker run line; the whole log is kept.
agent_call() {
  : >"$FAKE_DOCKER_LOG"
  "$BOX" stop "$1" >/dev/null 2>&1
  "$BOX" agent "$1" "$2" --detach >/dev/null 2>&1
  cp "$FAKE_DOCKER_LOG" "$TMP/agent-call.log"
  grep -e '--name agentbox-agent-' "$FAKE_DOCKER_LOG" | head -1
}

echo " an agent gets its own configuration volume"
CALL1="$(agent_call t1 personal)"
FIRST_LOG="$(cat "$TMP/agent-call.log")"
CALL2="$(agent_call t2 personal)"
assert_contains "t1 has a volume of its own for /acct" "-v agentbox-cfg-t1.personal:/acct" "$CALL1"
assert_contains "t2 has a volume of its own for /acct" "-v agentbox-cfg-t2.personal:/acct" "$CALL2"
assert_eq "the account volume is not mounted whole" "0" "$(grep -c -e '-v agentbox-acct-personal' <<<"$CALL1")"
assert_contains "the config directory is still /acct/claude" "CLAUDE_CONFIG_DIR=/acct/claude" "$CALL1"

echo " only the credential files come from the account"
assert_contains "claude credentials as one file" "--mount type=volume,src=agentbox-acct-personal,dst=/acct/claude/.credentials.json,volume-subpath=claude/.credentials.json" "$CALL1"
assert_contains "codex credentials as one file" "--mount type=volume,src=agentbox-acct-personal,dst=/acct/codex/auth.json,volume-subpath=codex/auth.json" "$CALL1"
assert_eq "exactly two files are shared" "2" "$(grep -o -e '--mount type=volume' <<<"$CALL1" | wc -l | tr -d ' ')"
export FAKE_ACCOUNT_FILES="claude"
CALL1="$(agent_call t1 personal)"
assert_eq "no codex login: the codex file is not mounted" "0" "$(grep -c -e 'codex/auth.json' <<<"$CALL1")"
assert_contains "claude credentials still are" "claude/.credentials.json" "$CALL1"

echo " the configuration volume is prepared so that the agent owns its directories"
assert_contains "created with the task label" "volume create --label agentbox=1 --label agentbox.task=t1 agentbox-cfg-t1.personal" "$FIRST_LOG"
assert_contains "directories made by the agent user" "mkdir -p /acct/claude /acct/codex" "$FIRST_LOG"

echo " an account without a credentials file cannot start an agent"
export FAKE_ACCOUNT_FILES=""
: >"$FAKE_DOCKER_LOG"
assert_status "refused" 1 "$BOX" agent t1 personal --detach
assert_contains "it says to log in" "box login personal" "$OUTPUT"
assert_eq "no agent container was started" "0" "$(grep -c -e '--name agentbox-agent-' "$FAKE_DOCKER_LOG")"
unset FAKE_ACCOUNT_FILES

echo " login and the account check still use the whole account volume"
: >"$FAKE_DOCKER_LOG"
"$BOX" login personal >/dev/null 2>&1
assert_contains "login" "-v agentbox-acct-personal:/acct" "$(grep -e 'agentbox.kind=login' "$FAKE_DOCKER_LOG")"

echo " removing a task removes its configuration volumes"
: >"$FAKE_DOCKER_LOG"
"$BOX" rm t1 --force >/dev/null 2>&1
assert_contains "the volumes are found by the task label" "volume ls -q --filter label=agentbox.task=t1" "$(cat "$FAKE_DOCKER_LOG")"
finish
