#!/bin/bash
# A subfolder is a path, never an option of git: a name that starts with a hyphen is refused,
# and every git call that takes the subfolder ends its options first.
. "$(dirname "$0")/lib.sh"
setup_sandbox
readonly OPTION_FOLDER="--stdin"

echo " a repository with a folder named like an option of git"
mkdir "$SOURCE/$OPTION_FOLDER"
echo x >"$SOURCE/$OPTION_FOLDER/file"
git -C "$SOURCE" add -- "$OPTION_FOLDER" && git -C "$SOURCE" -c user.name=t -c user.email=t@example.com commit -q -m "a folder named like an option"
printf '%s\n' "REPO=$SOURCE" "BASE=main" "SUBDIR=$OPTION_FOLDER" "VERB_check=true" >"$AGENTBOX_HOME/projects/dash.env"
OUTPUT="$(perl -e 'alarm 30; exec @ARGV' "$BOX" new t1 dash </dev/null 2>&1)"
assert_eq "box new is refused" "1" "$?"
assert_contains "one box: line that names the project" "box: project 'dash'" "$OUTPUT"
assert_contains "and the reason" "SUBDIR must not start with a hyphen" "$OUTPUT"
assert_no_file "nothing was created" "$AGENTBOX_HOME/work/t1"
assert_status "box project add with that subfolder is refused" 1 "$BOX" project add dash2 --repo "$SOURCE" --subdir "$OPTION_FOLDER" --check check=true
assert_no_file "no project is written" "$AGENTBOX_HOME/projects/dash2.env"

echo " a hyphen inside the path is a name like any other"
mkdir -p "$SOURCE/pkg/-inner"
echo '{}' >"$SOURCE/pkg/-inner/package.json"
git -C "$SOURCE" add -- pkg && git -C "$SOURCE" -c user.name=t -c user.email=t@example.com commit -q -m "a folder with a hyphen first, below another"
printf '%s\n' "REPO=$SOURCE" "BASE=main" "SUBDIR=pkg/-inner" "VERB_check=true" >"$AGENTBOX_HOME/projects/inner.env"
assert_status "box new" 0 "$BOX" new t2 inner
assert_file "its subfolder is checked out" "$AGENTBOX_HOME/work/t2/pkg/-inner/package.json"

echo " every sparse checkout ends git's options before the subfolder"
assert_eq "on the host" "1" "$(grep -c -e 'sparse-checkout set -- ' "$BOX_ROOT/lib/task.sh")"
assert_eq "and in the checkout container" "1" "$(grep -c -e 'sparse-checkout set -- ' "$BOX_ROOT/image/runner/checkout.sh")"
assert_eq "no call is left without it" "0" "$(grep -h -e 'sparse-checkout set' "$BOX_ROOT/lib/task.sh" "$BOX_ROOT/image/runner/checkout.sh" | grep -c -v -e 'set -- ')"
finish
