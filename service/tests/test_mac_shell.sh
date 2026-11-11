#!/bin/bash
# box shell <task> for a lavoro on the Mac opens the person's own login shell in the folder of
# the lavoro (the project's subfolder when it has one). It is the person's shell, not confined.
# box does not stay beside it and holds no lock while it is open; it keeps a record of it, so
# that box rm refuses while a terminal is open in the lavoro, and ends it with --force.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
export FAKE_DOCKER_DOWN=1
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
"$BOX" project add app --repo "$REPO" --subdir sub >/dev/null
"$BOX" project add whole --repo "$REPO" >/dev/null
"$BOX" new m1 app >/dev/null
"$BOX" new w1 whole >/dev/null
W="$AGENTBOX_HOME/work/m1"

# A shell that says where it was started and with what, and ends with a code of its own.
cat >"$TMP/quick-shell" <<SHELL
#!/bin/bash
{ echo "cwd=\$PWD"; echo "arguments=\$*"; echo "lock=\$( (: >&5) 2>/dev/null && echo open || echo closed)"; } >"$TMP/shell.said"
exit 7
SHELL
# A shell that stays until it is told to go, and says how it went.
cat >"$TMP/open-shell" <<SHELL
#!/bin/bash
trap 'echo hangup >"$TMP/shell.ended"; exit 0' HUP
echo "\$\$" >"$TMP/shell.pid"
while [ ! -e "$TMP/shell.leave" ]; do sleep 0.2; done
echo left >"$TMP/shell.ended"
SHELL
chmod +x "$TMP/quick-shell" "$TMP/open-shell"

echo " the person's shell, in the folder of the lavoro"
: >"$FAKE_DOCKER_LOG"
OUTPUT="$(SHELL="$TMP/quick-shell" "$BOX" shell m1 2>&1)"
assert_eq "box shell ends with the exit code of the shell" "7" "$?"
assert_eq "it printed nothing of its own" "" "$OUTPUT"
assert_eq "the shell was started in the project's folder of the worktree" "cwd=$W/sub" "$(sed -n 1p "$TMP/shell.said")"
assert_eq "as a login shell" "arguments=-l" "$(sed -n 2p "$TMP/shell.said")"
assert_eq "without the lifecycle lock of the task" "lock=closed" "$(sed -n 3p "$TMP/shell.said")"
assert_eq "Docker was not requested anything" "" "$(cat "$FAKE_DOCKER_LOG")"
SHELL="$TMP/quick-shell" "$BOX" shell w1 >/dev/null 2>&1
assert_eq "a project without a subfolder: the root of the worktree" "cwd=$AGENTBOX_HOME/work/w1" "$(sed -n 1p "$TMP/shell.said")"

echo " which shell: the person's, when it is an absolute path of a program, else zsh"
chosen() {
  AGENTBOX_HOME="$AGENTBOX_HOME" BOX_ROOT="$BOX_ROOT" SHELL="$1" bash -c '
    for lib in common names docker project project_detect project_admin sandbox accounts native native_commands native_read native_checks native_shell; do . "$BOX_ROOT/lib/$lib.sh"; done
    person_shell'
}
assert_eq "the person's" "$TMP/quick-shell" "$(chosen "$TMP/quick-shell")"
assert_eq "a name without a path" "$(usual_shell)" "$(chosen zsh)"
assert_eq "a path that is not a program" "$(usual_shell)" "$(chosen "$TMP/no-such-shell")"
assert_eq "none at all" "$(usual_shell)" "$(chosen "")"

echo " while a terminal is open"
# Headless launchers may ignore HUP; this fixture explicitly tests a default HUP handler.
SHELL="$TMP/open-shell" perl -e '$SIG{HUP} = "DEFAULT"; exec @ARGV' "$BOX" shell m1 >/dev/null 2>&1 &
for _ in $(seq 1 50); do [ -s "$TMP/shell.pid" ] && break; sleep 0.2; done
SHELL_PID="$(cat "$TMP/shell.pid")"
assert_eq "(the shell runs)" "1" "$(ps -p "$SHELL_PID" -o pid= | wc -l | tr -d ' ')"
assert_eq "box is not beside it: the shell took its place" "0" "$(pgrep -f "bin/box shell m1" | wc -l | tr -d ' ')"
assert_status "the lifecycle lock is free: Play status answers, a second terminal could open" 0 wait_for_started m1
assert_status "box rm is refused" 1 "$BOX" rm m1
assert_contains "one line says that a terminal is open" "a terminal is open" "$OUTPUT"
assert_eq "the shell still runs and the lavoro is there" "1 1" "$(ps -p "$SHELL_PID" -o pid= | wc -l | tr -d ' ') $([ -d "$W" ] && echo 1 || echo 0)"
assert_status "box rm --force ends the terminal and removes the lavoro" 0 "$BOX" rm m1 --force
assert_eq "the shell was told to hang up and is gone" "hangup 0" "$(cat "$TMP/shell.ended" 2>/dev/null) $(still_alive "$SHELL_PID")"
assert_no_file "the worktree is gone" "$W"

echo " a terminal that was closed, and a record that names another process"
"$BOX" new m2 app >/dev/null
rm -f "$TMP/shell.pid" "$TMP/shell.ended"
SHELL="$TMP/open-shell" perl -e '$SIG{HUP} = "DEFAULT"; exec @ARGV' "$BOX" shell m2 >/dev/null 2>&1 &
for _ in $(seq 1 50); do [ -s "$TMP/shell.pid" ] && break; sleep 0.2; done
SHELL_PID="$(cat "$TMP/shell.pid")"
: >"$TMP/shell.leave"
assert_eq "(the person closed it)" "0" "$(still_alive "$SHELL_PID")"
perl -MPOSIX -e 'setpgid(0, 0); sleep 300' &
VICTIM=$!
sleep 1
printf '%s\n%s\n%s\n' "$VICTIM" "1" "Thu Jan  1 00:00:00 1970" >"$AGENTBOX_HOME/shells/m2.$VICTIM"
assert_status "box rm is not held up by either" 0 "$BOX" rm m2
assert_eq "and the other process was not signalled" "1" "$(ps -p "$VICTIM" -o pid= | wc -l | tr -d ' ')"
kill "$VICTIM" 2>/dev/null

echo " a task in a box keeps its container shell"
unset FAKE_DOCKER_DOWN
"$BOX" new b1 app --where box >/dev/null 2>&1
: >"$FAKE_DOCKER_LOG"
SHELL="$TMP/quick-shell" "$BOX" shell b1 </dev/null >/dev/null 2>&1
assert_eq "docker is requested a container with the workspace" "1" "$(grep -c -e "^run .*work/b1:/work/b1" "$FAKE_DOCKER_LOG")"
finish
