# shellcheck shell=bash
# The accounts of lavori on the Mac: the Claude logins the person already has. Each is a config
# directory of the person, named in $AGENTBOX_HOME/config (ACCOUNT_<name>=<path>). The native
# agent is started with that directory by path (CLAUDE_CONFIG_DIR); box never opens a file
# inside it, and there is no sign-in step.

readonly CONFIG_FILE="$AGENTBOX_HOME/config"
readonly MAC_ACCOUNT_KEY_PATTERN='^ACCOUNT_[a-z0-9]+(-[a-z0-9]+)*$'
# The logins looked for when the config is first written: <name>=<directory in the home>. The
# usual login of Claude Code, unless BOX_FIRST_ACCOUNTS names others (for someone who keeps
# several: "personal=.claude work=.claude-work").
readonly FIRST_MAC_ACCOUNTS="${BOX_FIRST_ACCOUNTS:-claude=.claude}"

# Called at the start of every command: the first time, the config is written with the accounts
# whose directory exists. A config that exists is never changed by box.
ensure_config() {
  local pair staged
  [ ! -e "$CONFIG_FILE" ] && [ -n "${HOME:-}" ] || return 0
  mkdir -p "$AGENTBOX_HOME"
  staged="$(mktemp "$AGENTBOX_HOME/.config.XXXXXX")" || return 0
  {
    echo "# The Claude logins of the lavori on the Mac: ACCOUNT_<name>=<config directory>."
    echo "# Add a line for another login; box only ever passes the directory to the agent."
    for pair in $FIRST_MAC_ACCOUNTS; do
      if [ -d "$HOME/${pair#*=}" ]; then echo "ACCOUNT_${pair%%=*}=$HOME/${pair#*=}"; fi
    done
  } >"$staged"
  ln "$staged" "$CONFIG_FILE" 2>/dev/null || true
  rm -f "$staged"
}

# Prints "<name><tab><path>" for every account of the config, in its order. The file is read as
# data: a line that is not ACCOUNT_<name>=<absolute path without control characters> is skipped.
mac_accounts() {
  local line key value
  [ -f "$CONFIG_FILE" ] || return 0
  while IFS= read -r line || [ -n "$line" ]; do
    key="${line%%=*}"
    value="${line#*=}"
    [[ "$line" == *=* ]] && [[ "$key" =~ $MAC_ACCOUNT_KEY_PATTERN ]] || continue
    [[ "$value" == /* ]] && [[ "$value" != *[[:cntrl:]]* ]] && ! has_dot_component "$value" || continue
    printf '%s\t%s\n' "${key#ACCOUNT_}" "$value"
  done <"$CONFIG_FILE"
}

# Prints the config directory of an account of the Mac; fails when the config does not name it.
mac_account_directory() {
  local name path found=""
  # The whole list is read: leaving at the first match closes the pipe under its writer.
  while IFS=$'\t' read -r name path; do
    if [ "$name" = "$1" ] && [ -z "$found" ]; then found="$path"; fi
  done < <(mac_accounts)
  [ -n "$found" ] || return 1
  echo "$found"
}

# The accounts of the Mac as the elements of box accounts --json: the name and whether the
# directory exists. The directory itself is never printed, here or in a message. Nothing is
# known of a login without opening its files: loggedIn repeats exists (the app reads it as a
# boolean for every account) and authMethod is null.
mac_accounts_json() {
  local name path
  while IFS=$'\t' read -r name path; do
    jq -nc --arg name "$name" --argjson exists "$([ -d "$path" ] && echo true || echo false)" --arg at "$(now_iso)" \
      '{name: $name, where: "mac", exists: $exists, loggedIn: $exists, authMethod: null, checkedAt: $at}'
  done < <(mac_accounts)
}
