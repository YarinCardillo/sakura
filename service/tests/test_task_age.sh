#!/bin/bash
# Task ages survive reads and sessions, and checks never move the last agent turn.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
"$BOX" project add app --repo "$REPO" --subdir sub --install true >/dev/null
"$BOX" new m1 app >/dev/null
CREATED="$("$BOX" facts m1 --json | jq -r '.ageAt')"
assert_eq "an unstarted task has a creation timestamp" 1 "$(printf '%s' "$CREATED" | grep -Ec '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$')"
assert_eq "another launch keeps the creation time" "$CREATED" "$("$BOX" status --json | jq -r '.tasks[0].ageAt')"

# The proxy sees a pending prompt and its response, not arbitrary activity or check results.
cat >"$TMP/adapter" <<'PERL'
use JSON::PP;
$| = 1;
while (<STDIN>) {
    my $request = decode_json($_);
    print encode_json({jsonrpc => '2.0', id => 'unrelated', result => {}}), "\n";
    print encode_json({jsonrpc => '2.0', id => $request->{id}, result => {stopReason => 'end_turn'}}), "\n";
}
PERL
AGE="$AGENTBOX_HOME/tasks/m1.age"
printf '%s\n' '2000-01-01T00:00:00Z' >"$AGE"
run_proxy() {
  printf '%s\n' "$1" | BOX_ACP_AGE_FILE="$AGE" perl "$BOX_ROOT/lib/acp_proxy.pl" 10000 3 3 "$TMP/report" perl "$TMP/adapter" >"$TMP/answers"
}
run_proxy '{"jsonrpc":"2.0","id":1,"method":"initialize"}'
assert_eq "opening a session does not end a turn" '2000-01-01T00:00:00Z' "$(cat "$AGE")"
run_proxy '{"jsonrpc":"2.0","id":2,"method":"session/prompt"}'
ENDED="$(cat "$AGE")"
assert_eq "a prompt response records its own completion" 1 "$([ "$ENDED" != '2000-01-01T00:00:00Z' ] && echo 1 || echo 0)"
assert_eq "facts report the completed turn" "$ENDED" "$("$BOX" facts m1 --json | jq -r '.ageAt')"
assert_eq "status on relaunch reports the same turn" "$ENDED" "$("$BOX" status --json | jq -r '.tasks[0].ageAt')"

# Exercise the production native proxy wiring, without a real adapter or login.
mkdir -p "$TMP/login"
printf 'ACCOUNT_personal=%s\n' "$TMP/login" >"$AGENTBOX_HOME/config"
printf '%s\n' '2000-01-01T00:00:00Z' >"$AGE"
printf '%s\n' '{"jsonrpc":"2.0","id":3,"method":"session/prompt"}' | "$BOX" acp m1 personal >"$TMP/native.out" 2>"$TMP/native.err"
assert_eq "the native conversation records a turn even when it returns an error" 1 "$([ "$(cat "$AGE")" != '2000-01-01T00:00:00Z' ] && echo 1 || echo 0)"
# A terminal container gives its real finish, not the time status notices it.
TERMINAL="$(
  . "$BOX_ROOT/lib/report.sh"
  dk() { printf '%s\n' '2090-01-01T00:00:00.123456789Z'; }
  agent_container_name() { printf '%s\n' 'test-agent'; }
  TASKS_DIR="$AGENTBOX_HOME/tasks"
  task_age_at m1 '{"state":"exited"}'
)"
assert_eq "terminal completion uses Docker's finish time" '2090-01-01T00:00:00Z' "$TERMINAL"

# Exercise the production box proxy wiring with fake Docker only.
"$BOX" new b1 sandbox >/dev/null 2>&1
BOX_AGE="$AGENTBOX_HOME/tasks/b1.age"
printf '%s\n' '2000-01-01T00:00:00Z' >"$BOX_AGE"
printf '%s\n' '{"jsonrpc":"2.0","id":"box-turn","method":"session/prompt"}' | "$BOX" acp b1 personal >"$TMP/box.out" 2>"$TMP/box.err"
assert_eq "the box conversation records prompt completion" 1 "$([ "$(cat "$BOX_AGE")" != '2000-01-01T00:00:00Z' ] && echo 1 || echo 0)"

# An adapter exit ends an outstanding turn, including a failed or cancelled one.
printf '%s\n' '2000-01-01T00:00:00Z' >"$AGE"
cat >"$TMP/adapter" <<'PERL'
<STDIN>;
exit 3;
PERL
run_proxy '{"jsonrpc":"2.0","id":"interrupted","method":"session/prompt"}'
assert_eq "an interrupted turn has an end time" 1 "$([ "$(cat "$AGE")" != '2000-01-01T00:00:00Z' ] && echo 1 || echo 0)"
rm "$AGE"
assert_eq "older task creation is recovered from its journal" "$CREATED" "$("$BOX" facts m1 --json | jq -r '.ageAt')"
printf '%s\n' "$ENDED" >"$AGE"
"$BOX" rm m1 --force >/dev/null
assert_no_file "removal forgets the previous task's age" "$AGE"
finish
