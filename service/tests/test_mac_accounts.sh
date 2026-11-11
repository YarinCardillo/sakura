#!/bin/bash
# The accounts of the lavori on the Mac are the Claude logins the person already has: config
# directories named in $AGENTBOX_HOME/config, written on the first run from the directories
# that exist. box accounts --json lists them beside the accounts of the boxes; a project names
# its default account.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
export HOME="$TMP/person"
mkdir -p "$HOME/.claude" "$HOME/.claude-work"
echo "secret" >"$HOME/.claude/.credentials.json"
chmod 000 "$HOME/.claude/.credentials.json"
CONFIG="$AGENTBOX_HOME/config"
rm -f "$CONFIG"

mac_accounts() {
  "$BOX" accounts --json 2>/dev/null | jq -c '[.[] | select(.where == "mac") | {name, exists, loggedIn}]'
}

echo " the first run writes the config from the logins that exist"
assert_no_file "(no config yet)" "$CONFIG"
"$BOX" tasks --json >/dev/null
assert_eq "the usual login of Claude Code, when nothing names others" "ACCOUNT_claude=$HOME/.claude" "$(grep -v '^#' "$CONFIG")"
rm "$CONFIG"
# Someone who keeps several logins names them once, for the first run.
export BOX_FIRST_ACCOUNTS="personal=.claude work=.claude-work"
"$BOX" tasks --json >/dev/null
assert_file "the config is there after the first command" "$CONFIG"
assert_eq "with the two logins of the person" "ACCOUNT_personal=$HOME/.claude ACCOUNT_work=$HOME/.claude-work" "$(grep -v '^#' "$CONFIG" | tr '\n' ' ' | sed 's/ $//')"
rm "$CONFIG" && rmdir "$HOME/.claude-work"
"$BOX" tasks --json >/dev/null
assert_eq "a login whose directory does not exist is not written" "ACCOUNT_personal=$HOME/.claude" "$(grep -v '^#' "$CONFIG")"
echo "ACCOUNT_other=$TMP/elsewhere" >>"$CONFIG"
"$BOX" tasks --json >/dev/null
assert_contains "a config that exists is never rewritten" "ACCOUNT_other=$TMP/elsewhere" "$(cat "$CONFIG")"

echo " box accounts --json lists them with their place"
export FAKE_ACCOUNTS="shared"
OUT="$("$BOX" accounts --json)"
assert_eq "the accounts of the boxes say box" "shared:box" "$(jq -r '[.[] | select(.where == "box") | "\(.name):\(.where)"] | join(" ")' <<<"$OUT")"
assert_eq "and keep their fields" '["authMethod","checkedAt","loggedIn","name","where"]' "$(jq -c '[.[] | select(.where == "box")][0] | keys' <<<"$OUT")"
assert_eq "the accounts of the Mac, in the order of the config" \
  '[{"name":"personal","exists":true,"loggedIn":true},{"name":"other","exists":false,"loggedIn":false}]' "$(mac_accounts)"
assert_eq "loggedIn is a boolean that repeats exists, authMethod is null: box did not look into the login" "true null" "$(jq -r '[.[] | select(.name == "personal")][0] | "\(.loggedIn) \(.authMethod)"' <<<"$OUT")"
assert_eq "the fields of an account of the Mac: no path" '["authMethod","checkedAt","exists","loggedIn","name","where"]' "$(jq -c '[.[] | select(.where == "mac")][0] | keys' <<<"$OUT")"
: >"$FAKE_DOCKER_LOG"
OUT="$(FAKE_DOCKER_DOWN=1 "$BOX" accounts --json 2>"$TMP/accounts.err")"
assert_eq "without Docker the accounts of the Mac are still listed" "personal other" "$(jq -r '[.[].name] | join(" ")' <<<"$OUT")"
assert_contains "and one line says what is missing" "box: Docker is not reachable" "$(cat "$TMP/accounts.err")"

echo " the config is read as data"
cat >>"$CONFIG" <<LINES
ACCOUNT_relative=relative/path
ACCOUNT_Bad_Name=/tmp/x
ACCOUNT_dots=/tmp/../etc
PATH=/tmp/evil
ACCOUNT_cmd=\$(touch $TMP/pwned)
touch $TMP/pwned-line
ACCOUNT_spaces=$TMP/with space
LINES
assert_eq "only the lines that name an account and an absolute path count" "personal other spaces" "$("$BOX" accounts --json 2>/dev/null | jq -r '[.[] | select(.where == "mac") | .name] | join(" ")')"
mkdir -p "$TMP/with space"
assert_eq "a path with a space is kept whole (its directory is found)" "true" "$("$BOX" accounts --json 2>/dev/null | jq -r '.[] | select(.name == "spaces") | .exists')"
assert_eq "no config directory is printed, on stdout or stderr" "0" "$("$BOX" accounts --json 2>&1 | grep -c -e "$HOME" -e "$TMP")"
assert_eq "nothing in the file was run" "0" "$(/bin/ls "$TMP" | grep -c '^pwned')"

echo " a project names its default account"
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
assert_status "add with an account" 0 "$BOX" project add app --repo "$REPO" --account personal
assert_eq "projects --json carries it" "personal" "$("$BOX" projects --json | jq -r '.[] | select(.name == "app") | .account')"
assert_status "set changes it" 0 "$BOX" project set app --account work
assert_eq "to the other" "work" "$("$BOX" projects --json | jq -r '.[] | select(.name == "app") | .account')"
assert_status "a name that is not an account name is refused" 1 "$BOX" project set app --account "Not A Name"
assert_eq "a project without one says null" "null" "$("$BOX" projects --json | jq -r '.[] | select(.name == "sandbox") | .account')"
chmod 600 "$HOME/.claude/.credentials.json"
finish
