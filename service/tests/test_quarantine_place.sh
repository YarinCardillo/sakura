#!/bin/bash
# What the runner moves out of a drop directory goes into a folder that box reserved for it
# alone, under a fixed name. Nothing an agent made is ever a destination: a link that an agent
# got into quarantine earlier cannot steer a later move into a folder of the host. And nothing
# an agent made that is not a plain file is kept: a link, a FIFO or a directory is removed where
# it was put aside, without being followed or read; only its name and kind are written down.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
DROPS="$AGENTBOX_HOME/drop/t1"
QUARANTINE="$AGENTBOX_HOME/quarantine"
HOST_FOLDER="$TMP/hooks"
mkdir -p "$HOST_FOLDER" "$TMP/other"
echo "keep me" >"$TMP/other/victim.txt"

echo " a destination that an agent's link already holds (the name the earlier naming would pick)"
# The earlier naming was <task>-<epoch>-$RANDOM. With the clock and the seed fixed, the name is
# known beforehand; a link to a folder of the host stands there, as one quarantined earlier would.
in_box() {
  BOX_ROOT="$BOX_ROOT" bash -c '
    for lib in common names docker project project_detect project_admin sandbox accounts native native_commands native_read native_checks native_shell journal task request checks factscache cancel launchd runner; do . "$BOX_ROOT/lib/$lib.sh"; done
    now_epoch() { echo 1000; }
    '"$1" 2>/dev/null
}
EARLIER_NAME="t1-1000-$(bash -c 'RANDOM=7; echo "$RANDOM"')"
mkdir -p "$QUARANTINE"
ln -s "$HOST_FOLDER" "$QUARANTINE/$EARLIER_NAME"
echo "#!/bin/sh" >"$DROPS/post-checkout"
in_box 'RANDOM=7; quarantine_entry t1 "$DROP_DIR/t1/post-checkout" post-checkout "the drop is not empty"'
assert_eq "nothing appeared in the folder of the host" "0" "$(/bin/ls -A "$HOST_FOLDER" | wc -l | tr -d ' ')"
assert_no_file "the entry left the drop directory" "$DROPS/post-checkout"
rm -f "$QUARANTINE/$EARLIER_NAME"

echo " every kind of thing an agent can leave"
ln -s "$HOST_FOLDER" "$DROPS/build"
ln -s "$TMP/other/victim.txt" "$DROPS/lint"
mkfifo "$DROPS/deploy"
mkdir "$DROPS/tree"
ln -s "$TMP/other" "$DROPS/tree/inner"
echo "payload" >"$DROPS/tree/file"
echo "payload" >"$DROPS/results.json"
OUTPUT="$("$BOX" runner --once 2>&1)"
assert_eq "the runner ends cleanly" "0" "$?"
assert_eq "the drop directory is empty" "0" "$(/bin/ls -A "$DROPS" | wc -l | tr -d ' ')"
assert_eq "no link is kept anywhere in quarantine" "0" "$(find "$QUARANTINE" -type l | wc -l | tr -d ' ')"
assert_eq "no FIFO" "0" "$(find "$QUARANTINE" -type p | wc -l | tr -d ' ')"
assert_eq "every folder of quarantine is one box made: directly in it, nothing below" "0" "$(find "$QUARANTINE" -mindepth 2 -type d | wc -l | tr -d ' ')"
assert_eq "what the links led to is untouched" "keep me 0" "$(cat "$TMP/other/victim.txt") $(/bin/ls -A "$HOST_FOLDER" | wc -l | tr -d ' ')"
assert_eq "a plain file is kept, under the fixed name, in a folder of its own" "2" "$(find "$QUARANTINE" -mindepth 2 -maxdepth 2 -type f -name entry | wc -l | tr -d ' ')"
assert_eq "with its content" "payload" "$(cat "$QUARANTINE"/drop-t1.*/entry | sort -u | grep -x payload)"
ABOUT="$(cat "$QUARANTINE"/drop-t1.*/about)"
for pair in "build link" "lint link" "deploy fifo" "tree directory" "results.json file" "post-checkout file"; do
  assert_eq "written down: ${pair% *} was a ${pair#* }" "1" "$(grep -c -x "name=${pair% *} kind=${pair#* }" <<<"$ABOUT")"
done

echo " a request the runner cannot run"
commit_in_task t1 more
queue_directly t1 test 20270101T000000 "$(workspace_head t1)"
chmod 555 "$AGENTBOX_HOME/results/t1"
ln -s "$HOST_FOLDER" "$QUARANTINE/request-$QUEUED_KEY"
"$BOX" runner --once >/dev/null 2>&1
chmod 755 "$AGENTBOX_HOME/results/t1"
assert_no_file "it left the queue" "$AGENTBOX_HOME/queue/$QUEUED_KEY.req"
assert_eq "it is in a folder box reserved, under the fixed name" "1" "$(/bin/ls "$QUARANTINE"/request-"$QUEUED_KEY".*/request 2>/dev/null | wc -l | tr -d ' ')"
assert_eq "and not behind the link that held its earlier name" "0" "$(/bin/ls -A "$HOST_FOLDER" | wc -l | tr -d ' ')"
finish
