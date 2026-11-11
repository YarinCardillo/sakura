#!/bin/bash
# Failed startup must not signal a replacement process through a cached launcher number.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
"$BOX" project add app --repo "$SOURCE" --subdir sub --run 'sleep 300' --check 'test=true' >/dev/null
"$BOX" new m1 app >/dev/null
perl -MPOSIX=setsid -e 'setsid(); sleep 90' &
REPLACEMENT=$!
sleep 0.2
export REPLACEMENT BOX_ROOT
BOX_ROOT="$BOX_ROOT" bash -c '
  for lib in common names docker project project_detect project_admin sandbox accounts native native_commands native_read native_checks native_shell journal task request checks factscache cancel launchd runner status report session agent acp native_acp play; do . "$BOX_ROOT/lib/$lib.sh"; done
  load_task_project m1
  confirm_startup() { :; }
  process_start() { printf "identity-%s\n" "$1"; }
  wait_for_play() {
    # Collect the original owned launcher, then simulate reuse of the cached group number.
    kill -KILL -- "-$launcher" 2>/dev/null || kill -KILL "$launcher" 2>/dev/null
    wait "$launcher" 2>/dev/null
    launcher="$REPLACEMENT"
    return 1
  }
  play_start m1
' >"$TMP/play.out" 2>&1
assert_eq 'failed start reports failure' 1 "$?"
assert_eq 'the replacement process survives failed-start cleanup' 1 "$(ps -p "$REPLACEMENT" -o pid= | wc -l | tr -d ' ')"
kill -KILL -- "-$REPLACEMENT" 2>/dev/null
wait "$REPLACEMENT" 2>/dev/null
finish
