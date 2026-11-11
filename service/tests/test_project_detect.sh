#!/bin/bash
# box project detect <path> --json proposes a project from a folder the person chose: the
# repository, the subfolder, a name, the branches, and the install and check commands, protected
# paths and test file pattern it can infer, each with its reason. It writes nothing.
. "$(dirname "$0")/lib.sh"
setup_sandbox
readonly EXIT_NOT_A_FOLDER=3
readonly EXIT_NOT_A_REPOSITORY=4
readonly EXIT_BARE_REPOSITORY=5
readonly EXIT_INSIDE_DATA_DIRECTORY=6

# new_repo <directory>: an empty repository on branch main; commit_all <directory> commits everything.
new_repo() {
  git init -q -b main "$1"
}
commit_all() {
  git -C "$1" add -A
  git -C "$1" -c user.name=t -c user.email=t@example.com commit -q -m "${2:-files}"
}
detect() {
  "$BOX" project detect "$1" --json
}

echo " a node project in a subfolder of its repository"
NODE="$(cd "$TMP" && pwd -P)/web-repo"
new_repo "$NODE"
mkdir -p "$NODE/app/src" "$NODE/docs"
cat >"$NODE/app/package.json" <<'JSON'
{"name": "app", "scripts": {"test": "vitest run", "build": "tsc -b", "lint": "eslint .", "start": "node server.js"}}
JSON
echo '{}' >"$NODE/app/package-lock.json"
echo '{}' >"$NODE/app/tsconfig.json"
echo 'export default {}' >"$NODE/app/vitest.config.ts"
echo 'a' >"$NODE/app/src/a.spec.ts"
echo 'b' >"$NODE/app/src/b.spec.ts"
echo 'c' >"$NODE/app/src/c.ts"
commit_all "$NODE"
git -C "$NODE" branch feature/login
BEFORE="$(cd "$NODE" && git status --porcelain && /bin/ls -A app)"
OUT="$(detect "$NODE/app")"
assert_eq "exit 0 and one JSON object" "object" "$(jq -r type <<<"$OUT")"
assert_eq "the repository root" "$NODE" "$(jq -r .repo <<<"$OUT")"
assert_eq "the subfolder, relative to the root" "app" "$(jq -r .subdir <<<"$OUT")"
assert_eq "a name from the folder" "app" "$(jq -r .name <<<"$OUT")"
assert_eq "the current branch" "main" "$(jq -r .currentBranch <<<"$OUT")"
assert_eq "the default branch" "main" "$(jq -r .defaultBranch <<<"$OUT")"
assert_eq "the branches" '["feature/login","main"]' "$(jq -c '.branches | sort' <<<"$OUT")"
assert_eq "the install command of the lockfile's package manager" "npm ci" "$(jq -r .install.command <<<"$OUT")"
assert_contains "with its reason" "package-lock.json" "$(jq -r .install.reason <<<"$OUT")"
assert_eq "the scripts that are checks, not the others" '["build","lint","test"]' "$(jq -c '[.checks[].verb] | sort' <<<"$OUT")"
assert_eq "the test command" "npm test" "$(jq -r '.checks[] | select(.verb == "test") | .command' <<<"$OUT")"
assert_eq "the build command" "npm run build" "$(jq -r '.checks[] | select(.verb == "build") | .command' <<<"$OUT")"
assert_contains "each check says where it comes from" "vitest run" "$(jq -r '.checks[] | select(.verb == "test") | .reason' <<<"$OUT")"
assert_eq "protected paths for what exists in the folder" '["package-lock.json","package.json","tsconfig*.json","vitest*.config.*"]' "$(jq -c '[.protected[].pattern] | sort' <<<"$OUT")"
assert_eq "every protected path has a reason" "0" "$(jq '[.protected[] | select((.reason | length) == 0)] | length' <<<"$OUT")"
assert_eq "the test file pattern that matches tracked files" '["**/*.spec.ts"]' "$(jq -c '[.testFiles[].pattern]' <<<"$OUT")"
assert_contains "with how many" "2" "$(jq -r '.testFiles[0].reason' <<<"$OUT")"
assert_eq "nothing was written into the repository" "$BEFORE" "$(cd "$NODE" && git status --porcelain && /bin/ls -A app)"
assert_eq "and no project was created" "sandbox.env" "$(/bin/ls "$AGENTBOX_HOME/projects" | tr '\n' ' ' | sed 's/ $//')"
assert_eq "at the root of the repository the subfolder is empty" "" "$(detect "$NODE" | jq -r .subdir)"
assert_eq "and the name is the repository's" "web-repo" "$(detect "$NODE" | jq -r .name)"

echo " other package managers, by their lockfile"
PNPM="$TMP/pnpm-repo"
new_repo "$PNPM"
echo '{"scripts": {"test": "jest", "typecheck": "tsc --noEmit"}}' >"$PNPM/package.json"
echo 'lockfileVersion: 9' >"$PNPM/pnpm-lock.yaml"
commit_all "$PNPM"
OUT="$(detect "$PNPM")"
assert_eq "pnpm commands" '["pnpm run typecheck","pnpm test"]' "$(jq -c '[.checks[].command] | sort' <<<"$OUT")"
assert_eq "no install command box could prepare" "null" "$(jq -c .install <<<"$OUT")"
assert_contains "and a note that says why" "package-lock.json" "$(jq -r '.notes | join(" ")' <<<"$OUT")"
rm "$PNPM/pnpm-lock.yaml"; echo '# yarn' >"$PNPM/yarn.lock"
assert_eq "yarn commands" '["yarn run typecheck","yarn test"]' "$(detect "$PNPM" | jq -c '[.checks[].command] | sort')"

echo " a test script that would watch"
NG="$TMP/ng-repo"
new_repo "$NG"
echo '{"scripts": {"test": "ng test"}}' >"$NG/package.json"
echo '{}' >"$NG/package-lock.json"
echo '{}' >"$NG/angular.json"
commit_all "$NG"
OUT="$(detect "$NG")"
assert_eq "runs once" "npm test -- --watch=false" "$(jq -r '.checks[0].command' <<<"$OUT")"
assert_contains "angular.json is protected" "angular.json" "$(jq -c '[.protected[].pattern]' <<<"$OUT")"

echo " a Rust project and a Makefile"
RUST="$TMP/rust-repo"
new_repo "$RUST"
mkdir -p "$RUST/tests"
printf '[package]\nname = "x"\n' >"$RUST/Cargo.toml"
echo '# lock' >"$RUST/Cargo.lock"
echo 'fn main() {}' >"$RUST/tests/it.rs"
commit_all "$RUST"
OUT="$(detect "$RUST")"
assert_eq "cargo checks" '["cargo build","cargo test"]' "$(jq -c '[.checks[].command] | sort' <<<"$OUT")"
assert_eq "cargo files are protected" '["Cargo.lock","Cargo.toml"]' "$(jq -c '[.protected[].pattern] | sort' <<<"$OUT")"
assert_eq "its tests folder is the test pattern" '["tests/**"]' "$(jq -c '[.testFiles[].pattern]' <<<"$OUT")"
assert_contains "a note about the image" "cargo" "$(jq -r '.notes | join(" ")' <<<"$OUT")"
MAKE="$TMP/make-repo"
new_repo "$MAKE"
printf 'all:\n\ttrue\ntest:\n\ttrue\n' >"$MAKE/Makefile"
commit_all "$MAKE"
OUT="$(detect "$MAKE")"
assert_eq "the test target" '[{"verb":"test","command":"make test"}]' "$(jq -c '[.checks[] | {verb, command}]' <<<"$OUT")"
printf 'all:\n\ttrue\n' >"$MAKE/Makefile"
assert_eq "a Makefile without a test target proposes no check" "[]" "$(detect "$MAKE" | jq -c .checks)"

echo " a folder where nothing is recognised"
PLAIN="$TMP/plain-repo"
new_repo "$PLAIN"
echo text >"$PLAIN/notes.txt"
commit_all "$PLAIN"
OUT="$(detect "$PLAIN")"
assert_eq "no install, no checks, no paths" "null [] [] []" "$(jq -r '"\(.install) \(.checks) \(.protected) \(.testFiles)"' <<<"$OUT")"
assert_contains "and a note that the checks must be added by hand" "by hand" "$(jq -r '.notes | join(" ")' <<<"$OUT")"

echo " the name and the branches"
new_repo "$TMP/My App_2"
echo x >"$TMP/My App_2/file"
commit_all "$TMP/My App_2"
assert_eq "a name made of lowercase letters, digits and hyphens" "my-app-2" "$(detect "$TMP/My App_2" | jq -r .name)"
mkdir -p "$PLAIN/has space"
echo x >"$PLAIN/has space/file"
commit_all "$PLAIN" more
assert_status "a subfolder whose path box cannot use is refused" 1 "$BOX" project detect "$PLAIN/has space" --json
assert_contains "with one line that says so" "characters box does not accept" "$OUTPUT"
mkdir -p "$PLAIN/sandbox"; echo x >"$PLAIN/sandbox/file"; commit_all "$PLAIN" sandbox
assert_eq "a name that is taken gets a number" "sandbox-2" "$(detect "$PLAIN/sandbox" | jq -r .name)"
git -C "$PLAIN" branch develop
git -C "$PLAIN" update-ref refs/remotes/origin/develop HEAD
git -C "$PLAIN" symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/develop
assert_eq "the default branch is the one the remote calls default" "develop" "$(detect "$PLAIN" | jq -r .defaultBranch)"
for number in $(seq 1 35); do git -C "$PLAIN" branch "topic-$number"; done
assert_eq "at most 30 branches" "30" "$(detect "$PLAIN" | jq '.branches | length')"
git -C "$PLAIN" checkout -q --detach
assert_eq "no current branch on a detached head" "null" "$(detect "$PLAIN" | jq -c .currentBranch)"

echo " what is not a folder of a repository of the person"
assert_status "a path that does not exist" "$EXIT_NOT_A_FOLDER" "$BOX" project detect "$TMP/nowhere" --json
assert_contains "one box: line" "box: " "$OUTPUT"
assert_status "a file" "$EXIT_NOT_A_FOLDER" "$BOX" project detect "$PLAIN/notes.txt" --json
mkdir -p "$TMP/no-repo"
assert_status "a folder outside any repository" "$EXIT_NOT_A_REPOSITORY" "$BOX" project detect "$TMP/no-repo" --json
assert_contains "it says so" "not inside a git repository" "$OUTPUT"
git init -q --bare "$TMP/bare.git"
assert_status "a bare repository" "$EXIT_BARE_REPOSITORY" "$BOX" project detect "$TMP/bare.git" --json
assert_status "the inside of a .git folder" "$EXIT_NOT_A_REPOSITORY" "$BOX" project detect "$NODE/.git" --json
"$BOX" new t1 sandbox >/dev/null 2>&1
git -C "$AGENTBOX_HOME/work/t1" config core.fsmonitor "touch $TMP/git-ran-in-a-workspace"
assert_status "a workspace of a task" "$EXIT_INSIDE_DATA_DIRECTORY" "$BOX" project detect "$AGENTBOX_HOME/work/t1" --json
assert_contains "it says why" "data directory" "$OUTPUT"
ln -s "$AGENTBOX_HOME/work/t1/sub" "$TMP/link-to-workspace"
assert_status "a link that leads into the data directory" "$EXIT_INSIDE_DATA_DIRECTORY" "$BOX" project detect "$TMP/link-to-workspace" --json
assert_no_file "git never ran in the workspace" "$TMP/git-ran-in-a-workspace"
assert_status "without --json" 1 "$BOX" project detect "$NODE"
assert_eq "nothing on stdout when it fails" "" "$("$BOX" project detect "$TMP/nowhere" --json 2>/dev/null)"
finish
