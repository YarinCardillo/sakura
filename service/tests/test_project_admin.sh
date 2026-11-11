#!/bin/bash
# Projects are added, changed and removed with box, never by editing a file: box project add,
# set and rm, box projects --json (box profiles --json is the same) and box branches.
. "$(dirname "$0")/lib.sh"
setup_sandbox
REPO="$(cd "$SOURCE" && pwd -P)"
git -C "$REPO" branch develop
mkdir -p "$REPO/docs"; echo text >"$REPO/docs/readme.md"
git -C "$REPO" add -A && git -C "$REPO" -c user.name=t -c user.email=t@example.com commit -q -m docs
project() {
  "$BOX" projects --json | jq -c --arg name "$1" '.[] | select(.name == $name)'
}

echo " box project add writes a project from the person's choices"
assert_status "add" 0 "$BOX" project add web --repo "$REPO" --subdir sub --base develop --install "npm ci" \
  --check "test=npm test -- --watch=false" --check "build=npm run build && echo 'done \$HOME'" \
  --protected "package.json package-lock.json" --test-files "**/*.spec.ts"
assert_file "the project is a file of the data directory" "$PROJECTS_DIR/web.env"
P="$(project web)"
assert_eq "repository, subfolder, base, install" "$REPO sub develop npm ci" "$(jq -r '"\(.repo) \(.subdir) \(.base) \(.install)"' <<<"$P")"
assert_eq "the checks, in the order of their names" '[{"verb":"build","command":"npm run build && echo '"'"'done $HOME'"'"'"},{"verb":"test","command":"npm test -- --watch=false"}]' "$(jq -c .checks <<<"$P")"
assert_eq "protected paths and test files" "package.json package-lock.json|**/*.spec.ts" "$(jq -r '"\(.protected)|\(.testFiles)"' <<<"$P")"
assert_eq "no task uses it yet, and it has no error" '[] null' "$(jq -r '"\(.tasks) \(.error)"' <<<"$P")"
assert_status "a task can be created from it" 0 "$BOX" new w1 web
assert_eq "the task is listed with the project" '["w1"]' "$(project web | jq -c .tasks)"
assert_contains "the command was stored as text" "VERB_build=npm run build && echo 'done \$HOME'" "$(cat "$PROJECTS_DIR/web.env")"

echo " the base defaults to the repository's default branch, the rest to nothing"
assert_status "add with a repository only" 0 "$BOX" project add bare-minimum --repo "$REPO"
P="$(project bare-minimum)"
assert_eq "base, subfolder, install, checks" 'main||null|[]' "$(jq -r '"\(.base)|\(.subdir)|\(.install)|\(.checks)"' <<<"$P")"

echo " what add refuses"
# assert_add_refused <name of the case> <words of the message> <arguments of box project add...>
assert_add_refused() {
  local name="$1" words="$2"
  shift 2
  assert_status "$name" 1 "$BOX" project add "$@"
  assert_contains "  one box: line" "box: " "$OUTPUT"
  assert_contains "  that says why" "$words" "$OUTPUT"
}
assert_add_refused "a name in use" "already exists" web --repo "$REPO"
assert_add_refused "an invalid name" "invalid project name" "Web App" --repo "$REPO"
assert_add_refused "no repository" "--repo" nothing-given
assert_add_refused "a repository that does not exist" "is not a folder" p1 --repo "$TMP/nowhere"
assert_add_refused "a folder that is not the root of a repository" "root" p1 --repo "$REPO/sub"
assert_add_refused "a repository inside the data directory" "data directory" p1 --repo "$AGENTBOX_HOME/work/w1"
assert_add_refused "a subfolder that does not exist" "subfolder" p1 --repo "$REPO" --subdir missing
assert_add_refused "a subfolder that climbs out" "SUBDIR" p1 --repo "$REPO" --subdir "../other"
assert_add_refused "a base branch that does not exist" "branch" p1 --repo "$REPO" --base nope
assert_add_refused "a check whose verb is not a verb" "verb" p1 --repo "$REPO" --check "Test=npm test"
assert_add_refused "a check without a command" "verb=command" p1 --repo "$REPO" --check "test"
assert_add_refused "the same verb twice" "twice" p1 --repo "$REPO" --check "test=a" --check "test=b"
assert_add_refused "a control character in a command" "control character" p1 --repo "$REPO" --install "npm ci"$'\x07'
assert_add_refused "an unknown option" "unknown option" p1 --repo "$REPO" --preset builder
assert_add_refused "an option without a value" "needs a value" p1 --repo
assert_eq "nothing was created by the refused calls" "0" "$(/bin/ls "$PROJECTS_DIR" | grep -c '^p1\|^nothing')"

echo " box project set changes fields of a project"
assert_status "set the base and one check, remove another" 0 "$BOX" project set web --base main --check "lint=npm run lint" --check "build="
P="$(project web)"
assert_eq "the base changed" "main" "$(jq -r .base <<<"$P")"
assert_eq "one check added, one removed, one kept" '["lint","test"]' "$(jq -c '[.checks[].verb]' <<<"$P")"
assert_eq "the rest is as it was" "sub|npm ci" "$(jq -r '"\(.subdir)|\(.install)"' <<<"$P")"
assert_status "the repository of a project with tasks cannot be changed" 1 "$BOX" project set web --repo "$REPO"
assert_contains "it says which tasks" "w1" "$OUTPUT"
assert_status "nor its subfolder" 1 "$BOX" project set web --subdir docs
assert_status "a project that does not exist" 1 "$BOX" project set nope --base main
assert_status "set validates like add" 1 "$BOX" project set web --base nope
assert_status "a project without tasks can move to another subfolder" 0 "$BOX" project set bare-minimum --subdir docs
assert_eq "it did" "docs" "$(project bare-minimum | jq -r .subdir)"

echo " box project rm"
assert_status "refused while a task uses the project" 1 "$BOX" project rm web
assert_contains "it names the task" "w1" "$OUTPUT"
assert_file "the project is still there" "$PROJECTS_DIR/web.env"
assert_status "a project nobody uses is removed" 0 "$BOX" project rm bare-minimum
assert_no_file "its file is gone" "$PROJECTS_DIR/bare-minimum.env"
assert_status "removing it again fails" 1 "$BOX" project rm bare-minimum
"$BOX" rm w1 --force >/dev/null 2>&1
assert_status "once its task is gone the project can be removed" 0 "$BOX" project rm web

echo " box projects --json and its old name"
OUT="$("$BOX" projects --json)"
assert_eq "an array, sorted by name" '["sandbox"]' "$(jq -c '[.[].name]' <<<"$OUT")"
assert_eq "box profiles --json is the same" "$OUT" "$("$BOX" profiles --json)"
printf 'REPO=%s\nBASE=main\nSHELL=/bin/sh\n' "$REPO" >"$PROJECTS_DIR/broken.env"
OUT="$("$BOX" projects --json)"
assert_eq "a file that is refused is listed with the reason, not hidden" "unknown key 'SHELL'" "$(jq -r '.[] | select(.name == "broken") | .error' <<<"$OUT")"
assert_eq "and the others are still listed" "null" "$(jq -r '.[] | select(.name == "sandbox") | .error' <<<"$OUT")"
rm -f "$PROJECTS_DIR/broken.env"

echo " box branches <project> --json"
OUT="$("$BOX" branches sandbox --json)"
assert_eq "the project and its default branch" "sandbox main" "$(jq -r '"\(.project) \(.default)"' <<<"$OUT")"
assert_eq "the local branches" '["develop","main"]' "$(jq -c '[.branches[].name] | sort' <<<"$OUT")"
assert_eq "each with its commit and time" "$(git -C "$REPO" rev-parse main)" "$(jq -r '.branches[] | select(.name == "main") | .sha' <<<"$OUT")"
assert_eq "times are UTC" "1" "$(jq -r '.branches[0].time' <<<"$OUT" | grep -c -E '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:]{8}Z$')"
assert_status "a project that does not exist" 1 "$BOX" branches nope --json

echo " a project without checks: its tasks say so"
"$BOX" project add no-checks --repo "$REPO" --test-files "**/*.spec.ts" >/dev/null
"$BOX" new n1 no-checks >/dev/null
"$BOX" new s1 sandbox >/dev/null
OUT="$("$BOX" status --json 2>/dev/null)"
assert_eq "status: no check to verify" '[]' "$(jq -c '.tasks[] | select(.name == "n1") | .checks' <<<"$OUT")"
assert_eq "status: the checks of a project that has some" '["build","test"]' "$(jq -c '.tasks[] | select(.name == "s1") | .checks' <<<"$OUT")"
assert_eq "status names the project" "no-checks" "$(jq -r '.tasks[] | select(.name == "n1") | .project' <<<"$OUT")"
assert_eq "box facts says the same" '[]' "$("$BOX" facts n1 --json 2>/dev/null | jq -c .checks)"
assert_status "there is no verb to verify" 1 "$BOX" verify n1 test --no-wait
finish
