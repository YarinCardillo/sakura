#!/bin/bash
# box runner --install / --uninstall: a LaunchAgent for the current user. The tests use a fake
# launchctl and a private LaunchAgents directory; nothing is loaded into the real launchd.
. "$(dirname "$0")/lib.sh"
setup_sandbox
export BOX_LAUNCH_AGENTS_DIR="$TMP/LaunchAgents"
export FAKE_LAUNCHCTL_LOG="$TMP/launchctl.log"
: >"$FAKE_LAUNCHCTL_LOG"
LABEL="com.agentbox.runner"
PLIST="$BOX_LAUNCH_AGENTS_DIR/$LABEL.plist"

echo " the template in the repository"
assert_file "template exists" "$BOX_ROOT/launchd/$LABEL.plist.template"

echo " --install writes the plist and loads it for the current user"
assert_status "install" 0 "$BOX" runner --install
assert_file "plist written" "$PLIST"
# The plist is read with Python's plistlib, which every system of the tests has (plutil is macOS only).
plist_value() {
  python3 -B -c 'import plistlib, sys
value = plistlib.load(open(sys.argv[1], "rb"))
for key in sys.argv[2].split("."):
    value = value[int(key)] if isinstance(value, list) else value[key]
print(str(value).lower() if isinstance(value, bool) else value)' "$PLIST" "$1"
}
assert_eq "plist is valid" "0" "$(plist_value Label >/dev/null 2>&1; echo $?)"
assert_eq "label" "$LABEL" "$(plist_value Label)"
assert_eq "runs bin/box runner (not --daemon)" "$BOX_ROOT/bin/box runner" "$(plist_value ProgramArguments.0) $(plist_value ProgramArguments.1)"
assert_eq "the data directory is passed on" "$AGENTBOX_HOME" "$(plist_value EnvironmentVariables.AGENTBOX_HOME)"
assert_eq "it starts at login" "true" "$(plist_value RunAtLoad)"
assert_eq "it is restarted after a crash" "false" "$(plist_value KeepAlive.SuccessfulExit)"
assert_eq "log goes to runner.log" "$AGENTBOX_HOME/runner.log" "$(plist_value StandardErrorPath)"
assert_eq "no placeholder is left" "0" "$(grep -c '@[A-Z_]*@' "$PLIST")"
assert_contains "docker is on the PATH it gets" "$(dirname "$(command -v docker)")" "$(plist_value EnvironmentVariables.PATH)"
assert_contains "launchctl bootstrap for this user" "bootstrap gui/$(id -u) $PLIST" "$(cat "$FAKE_LAUNCHCTL_LOG")"

echo " installing again replaces the loaded agent"
: >"$FAKE_LAUNCHCTL_LOG"
assert_status "second install" 0 "$BOX" runner --install
assert_contains "the old one is unloaded first" "bootout gui/$(id -u)/$LABEL" "$(head -1 "$FAKE_LAUNCHCTL_LOG")"
assert_contains "then loaded" "bootstrap gui/$(id -u) $PLIST" "$(tail -1 "$FAKE_LAUNCHCTL_LOG")"

echo " --install refuses while a runner is already running"
rm -f "$PLIST"
mkdir -p "$AGENTBOX_HOME/locks"
perl -MFcntl=:flock -e 'open(my $f, ">>", $ARGV[0]) or die; flock($f, LOCK_EX) or die; print $f "$$\n"; sleep 6' "$AGENTBOX_HOME/locks/runner.lock" &
HOLDER=$!
sleep 1
: >"$FAKE_LAUNCHCTL_LOG"
assert_status "refused" 1 "$BOX" runner --install
assert_contains "it says how to stop the runner" "box runner --stop" "$OUTPUT"
assert_no_file "nothing was written" "$PLIST"
assert_eq "launchctl was not called" "0" "$(wc -c <"$FAKE_LAUNCHCTL_LOG" | tr -d ' ')"
kill "$HOLDER" 2>/dev/null; wait "$HOLDER" 2>/dev/null

echo " --uninstall unloads and removes it"
assert_status "install again" 0 "$BOX" runner --install
: >"$FAKE_LAUNCHCTL_LOG"
assert_status "uninstall" 0 "$BOX" runner --uninstall
assert_no_file "plist removed" "$PLIST"
assert_contains "launchctl bootout" "bootout gui/$(id -u)/$LABEL" "$(cat "$FAKE_LAUNCHCTL_LOG")"
assert_status "uninstall when nothing is installed" 0 "$BOX" runner --uninstall
assert_contains "it says so" "not installed" "$OUTPUT"

echo " a path that cannot go into the plist is refused"
mkdir -p "$TMP/we&ird"
assert_status "odd data directory" 1 env AGENTBOX_HOME="$TMP/we&ird" "$BOX" runner --install
finish
