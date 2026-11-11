# shellcheck shell=bash
# box runner --install / --uninstall: the runner as a LaunchAgent of the current user, so that it
# starts at login and again after a crash. The plist is generated from launchd/*.plist.template.

readonly LAUNCHD_LABEL="com.agentbox.runner"
readonly LAUNCHD_TEMPLATE="$BOX_ROOT/launchd/$LAUNCHD_LABEL.plist.template"
readonly LAUNCH_AGENTS_DIR="${BOX_LAUNCH_AGENTS_DIR:-$HOME/Library/LaunchAgents}"
# Characters that would break the XML or the substitution.
readonly PLIST_UNSAFE_PATTERN='[&<>"'"'"'|\\]'

launchd_plist_path() {
  echo "$LAUNCH_AGENTS_DIR/$LAUNCHD_LABEL.plist"
}

launchd_domain() {
  echo "gui/$(id -u)"
}

# Writes the plist and loads it. Refuses while a runner is running (a second runner would only
# fail on the lock and be restarted every 30 seconds): stop it first.
runner_install() {
  local plist
  command -v launchctl >/dev/null 2>&1 || die "box runner --install is for macOS (it writes a LaunchAgent); on this system start the runner with: box runner --daemon"
  [ -f "$LAUNCHD_TEMPLATE" ] || die "the LaunchAgent template is missing: $LAUNCHD_TEMPLATE"
  runner_is_running && die "a runner is running (pid $(runner_pid)); stop it first with: box runner --stop"
  plist="$(launchd_plist_path)"
  mkdir -p "$LAUNCH_AGENTS_DIR" "$AGENTBOX_HOME"
  render_plist >"$plist.tmp" || { rm -f "$plist.tmp"; die "could not write the LaunchAgent"; }
  mv "$plist.tmp" "$plist"
  launchctl bootout "$(launchd_domain)/$LAUNCHD_LABEL" >/dev/null 2>&1 || true
  launchctl bootstrap "$(launchd_domain)" "$plist" || die "launchctl could not load $plist"
  echo "runner installed as $LAUNCHD_LABEL ($plist); it starts at login, log: $RUNNER_LOG"
}

# Unloads and removes the LaunchAgent. Does not stop a runner started by hand.
runner_uninstall() {
  local plist
  plist="$(launchd_plist_path)"
  if [ ! -f "$plist" ]; then echo "the runner is not installed as a LaunchAgent"; return 0; fi
  launchctl bootout "$(launchd_domain)/$LAUNCHD_LABEL" >/dev/null 2>&1 || true
  rm -f "$plist"
  echo "runner uninstalled ($LAUNCHD_LABEL removed)"
}

# Prints the template with the paths of this installation. Values with characters that do not
# survive XML or the substitution are refused.
render_plist() {
  local value
  for value in "$BOX_ROOT/bin/box" "$AGENTBOX_HOME" "$PATH" "$RUNNER_LOG"; do
    if [[ "$value" =~ $PLIST_UNSAFE_PATTERN ]]; then
      echo "box: '$value' contains a character that cannot go into a LaunchAgent" >&2
      return 1
    fi
  done
  BOX_BIN_VALUE="$BOX_ROOT/bin/box" HOME_VALUE="$AGENTBOX_HOME" PATH_VALUE="$PATH" LOG_VALUE="$RUNNER_LOG" \
    perl -pe 's/\@BOX_BIN\@/$ENV{BOX_BIN_VALUE}/g; s/\@AGENTBOX_HOME\@/$ENV{HOME_VALUE}/g; s/\@PATH\@/$ENV{PATH_VALUE}/g; s/\@LOG\@/$ENV{LOG_VALUE}/g' \
    "$LAUNCHD_TEMPLATE"
}
