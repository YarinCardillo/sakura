#!/bin/bash
# Play: the run command of a project for a lavoro on the Mac. It runs detached, in the folder
# of the project, in a process group of its own, with a range of ports reserved for the lavoro,
# its output in a bounded log. stop and box rm end the whole group; a second Play of the same
# project stops the first unless the project says many. In a box Play is not built yet.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
readonly PLAY_IN_BOX_EXIT=81
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
# A run command that says what it was given, starts a child of its own and stays.
RUN_COMMAND="echo \"play of \$SAKURA_LAVORO on \$SAKURA_PORT and \$CONDUCTOR_PORT in \$PWD\"; sh -c 'echo \$\$ >\"$TMP/\"'\$SAKURA_LAVORO'.child; exec sleep 300' & wait"

is_alive() {
  ps -p "$1" -o pid= 2>/dev/null | wc -l | tr -d ' '
}

# wait_for_file <file>: until it has content, at most 10 s.
wait_for_file() {
  local tick
  for tick in $(seq 1 50); do [ -s "$1" ] && return 0; sleep 0.2; done
  return 1
}

status_of() {
  "$BOX" play "$1" status --json
}

echo " a project names its run command, how many ports and whether Plays may run together"
assert_status "add" 0 "$BOX" project add app --repo "$REPO" --subdir sub
assert_eq "the defaults: no command, 10 ports, one at a time" "null 10 one" "$("$BOX" projects --json | jq -r '.[] | select(.name == "app") | "\(.run) \(.ports) \(.runMode)"')"
assert_status "set --run" 0 "$BOX" project set app --run "$RUN_COMMAND"
for bad in 0 101 many -3; do assert_status "--ports $bad is refused" 1 "$BOX" project set app --ports "$bad"; done
assert_status "--run-mode both is refused" 1 "$BOX" project set app --run-mode both
"$BOX" new m1 app >/dev/null
"$BOX" new m2 app >/dev/null
W="$AGENTBOX_HOME/work/m1"

echo " before the first start"
assert_eq "status: not running, no ports yet" '{"task":"m1","running":false,"pid":null,"ports":[],"since":null}' "$(status_of m1)"
assert_status "the log: nothing yet" 3 "$BOX" play m1 log

echo " start: detached, in the folder of the project, with its ports"
: >"$FAKE_DOCKER_LOG"
STARTED=$SECONDS
OUT="$("$BOX" play m1 start --json 2>"$TMP/start.err")"
assert_eq "exit 0" "0" "$?"
assert_eq "nothing on stderr" "" "$(cat "$TMP/start.err")"
assert_eq "box returns at once (after $((SECONDS - STARTED)) s)" "1" "$([ $((SECONDS - STARTED)) -le 20 ] && echo 1 || echo 0)"
assert_eq "running, with ten ports from 56000" "true 56000 56009 10" "$(jq -r '"\(.running) \(.ports[0]) \(.ports[-1]) \(.ports | length)"' <<<"$OUT")"
assert_eq "the keys of the status" '["pid","ports","running","since","task"]' "$(jq -c 'keys' <<<"$OUT")"
assert_eq "since is a time in UTC" "1" "$(jq -r .since <<<"$OUT" | grep -c -E '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:]{8}Z$')"
PID="$(jq -r .pid <<<"$OUT")"
assert_eq "the pid is a process that lives after box ended" "1" "$(is_alive "$PID")"
assert_eq "it leads a process group of its own" "$PID" "$(ps -p "$PID" -o pgid= | tr -d ' ')"
wait_for_file "$TMP/m1.child"
assert_contains "the log: the port under both names, the folder of the project" "play of m1 on 56000 and 56000 in $W/sub" "$("$BOX" play m1 log)"
assert_eq "the log is not in the worktree" "0" "$(find "$W" -name '*.log' | wc -l | tr -d ' ')"
assert_eq "Docker was not requested anything" "" "$(cat "$FAKE_DOCKER_LOG")"
assert_eq "a second start leaves the one that runs" "$PID" "$("$BOX" play m1 start --json | jq -r .pid)"
assert_contains "status without --json is one line for people" "runs" "$("$BOX" play m1 status)"

echo " one at a time: starting the Play of another lavoro of the project stops this one"
CHILD_ONE="$(cat "$TMP/m1.child")"
OUT="$("$BOX" play m2 start --json)"
assert_eq "the second runs, on the next range" "true 56010" "$(jq -r '"\(.running) \(.ports[0])"' <<<"$OUT")"
assert_eq "the first does not run any more" "false" "$(status_of m1 | jq -r .running)"
assert_eq "nor does what it started" "0 0" "$(still_alive "$PID") $(still_alive "$CHILD_ONE")"
assert_eq "its ports stay its own" "56000" "$(status_of m1 | jq -r '.ports[0]')"

echo " stop ends the whole group"
wait_for_file "$TMP/m2.child"
CHILD_TWO="$(cat "$TMP/m2.child")"
PID_TWO="$(jq -r .pid <<<"$OUT")"
OUT="$("$BOX" play m2 stop --json)"
assert_eq "stop: not running" "false null null" "$(jq -r '"\(.running) \(.pid) \(.since)"' <<<"$OUT")"
assert_eq "the command and the child it started are gone" "0 0" "$(still_alive "$PID_TWO") $(still_alive "$CHILD_TWO")"
assert_status "a second stop is not an error" 0 "$BOX" play m2 stop
assert_eq "a restart keeps the range of the lavoro" "56010" "$("$BOX" play m2 start --json | jq -r '.ports[0]')"
"$BOX" play m2 stop >/dev/null

echo " many: the Plays of a project run together"
"$BOX" project add many --repo "$REPO" --subdir sub --run "$RUN_COMMAND" --run-mode many --ports 3 >/dev/null
"$BOX" new n1 many >/dev/null
"$BOX" new n2 many >/dev/null
"$BOX" play n1 start >/dev/null
"$BOX" play n2 start >/dev/null
assert_eq "both run" "true true" "$(status_of n1 | jq -r .running) $(status_of n2 | jq -r .running)"
assert_eq "each on its own three ports, after the ranges already given" "[56020,56021,56022] [56023,56024,56025]" "$(status_of n1 | jq -c .ports) $(status_of n2 | jq -c .ports)"

echo " box rm stops the Play and gives the ports back"
wait_for_file "$TMP/n1.child"
CHILD="$(cat "$TMP/n1.child")"
PID="$(status_of n1 | jq -r .pid)"
assert_status "box rm" 0 "$BOX" rm n1 --force
assert_eq "nothing of its Play is left" "0 0" "$(still_alive "$PID") $(still_alive "$CHILD")"
assert_eq "the other lavoro's Play still runs" "true" "$(status_of n2 | jq -r .running)"
"$BOX" new n3 many >/dev/null
assert_eq "a new lavoro gets the range that came free" "56020" "$("$BOX" play n3 start --json | jq -r '.ports[0]')"
"$BOX" play n2 stop >/dev/null
"$BOX" play n3 stop >/dev/null

echo " a command that ends by itself, and one that floods its log"
"$BOX" project add short --repo "$REPO" --subdir sub --run 'echo done' >/dev/null
"$BOX" new s1 short >/dev/null
assert_status "a run command that ends at once: start says so" 1 "$BOX" play s1 start
assert_contains "and where its output is" "box play s1 log" "$OUTPUT"
assert_contains "the output is there" "done" "$("$BOX" play s1 log)"
assert_eq "it does not run" "false" "$(status_of s1 | jq -r .running)"
"$BOX" project add loud --repo "$REPO" --subdir sub --run 'yes flood | head -c 3000000; sleep 300' >/dev/null
"$BOX" new l1 loud >/dev/null
BOX_PLAY_MAX_LOG_BYTES=100000 "$BOX" play l1 start >/dev/null
for _ in $(seq 1 50); do grep -q 'log cut' "$AGENTBOX_HOME/play/l1.log" 2>/dev/null && break; sleep 0.2; done
SIZE="$(wc -c <"$AGENTBOX_HOME/play/l1.log" | tr -d ' ')"
assert_eq "3 MB of output leave a log at its limit (got $SIZE bytes)" "1" "$([ "$SIZE" -le 100200 ] && echo 1 || echo 0)"
assert_eq "and the Play goes on" "true" "$(status_of l1 | jq -r .running)"
"$BOX" play l1 stop >/dev/null

echo " what Play refuses"
"$BOX" project add quiet --repo "$REPO" --subdir sub >/dev/null
"$BOX" new q1 quiet >/dev/null
assert_status "a project without a run command" 1 "$BOX" play q1 start
assert_contains "it says how to give it one" "box project set quiet --run" "$OUTPUT"
"$BOX" new b1 app --where box >/dev/null 2>&1
for action in start stop status log; do
  assert_status "in a box: play $action" "$PLAY_IN_BOX_EXIT" "$BOX" play b1 "$action"
  assert_eq "  one line" "box: Play in a box is not built yet (task 'b1' is in a box)" "$OUTPUT"
done
assert_status "an action that does not exist" 1 "$BOX" play m1 restart
assert_status "a task that does not exist" 1 "$BOX" play nope start
for _ in $(seq 1 75); do [ -z "$(pgrep -f "$TMP")" ] && break; sleep 0.2; done
assert_eq "nothing of the Plays of this test is left running" "0" "$(pgrep -f "$TMP" | wc -l | tr -d ' ')"
finish
