#!/usr/bin/env bash
# A digest collector for one GitHub repository, built on the gh CLI (signed in).
#
# Usage: github.sh OWNER/NAME BRANCH
#
# It prints, as the digest protocol asks (see README.md), one JSON object per line for what changed since
# SAKURA_DIGEST_SINCE: pull requests, issues, reviews and comments on your own open pull requests, your new
# assignments, commits and failing checks on BRANCH, and whether each of your open pull requests is behind its
# base branch or conflicts with it, as GitHub computes it (mergeable and mergeStateStatus, and the compare API for
# the count). A pull request GitHub is still computing (UNKNOWN) is left out this run. The last line is a cursor
# that remembers each pull request's state.
#
# It only reads from GitHub: no clone, no checkout, nothing written outside the output. Needs python3 to write
# the JSON lines.
set -euo pipefail

REPO=${1:?usage: github.sh OWNER/NAME BRANCH}
BRANCH=${2:?usage: github.sh OWNER/NAME BRANCH}
SINCE=${SAKURA_DIGEST_SINCE:?this program is run by sakura digest}
CURSOR=${SAKURA_DIGEST_CURSOR:-}
SINCE_Z=${SINCE/+00:00/Z}

# A failed query ends the program with no cursor, so Sakura keeps its place and the day is not marked done. The
# answers of the queries that loop over results are captured first, because a failure inside `$(...)` or `<(...)`
# does not stop a script by itself.
fail() {
  echo "github.sh: $1" >&2
  exit 1
}

emit() {  # id title url at text
  python3 -c 'import json, sys; print(json.dumps(dict(zip(("id", "title", "url", "at", "text"), sys.argv[1:6]))))' "$@"
}

pull_requests() {
  gh pr list -R "$REPO" --state all --search "updated:>=$SINCE_Z" --limit 30 \
    --json number,title,url,updatedAt,state,author \
    --jq '.[] | {id: ("pr-\(.number)-\(.updatedAt)"), title: ("PR #\(.number): \(.title)"), url: .url, at: .updatedAt, text: "\(.state) by \(.author.login)"}'
}

issues() {
  gh issue list -R "$REPO" --state all --search "updated:>=$SINCE_Z" --limit 30 \
    --json number,title,url,updatedAt,state,author \
    --jq '.[] | {id: ("issue-\(.number)-\(.updatedAt)"), title: ("Issue #\(.number): \(.title)"), url: .url, at: .updatedAt, text: "\(.state) by \(.author.login)"}'
}

assignments() {
  local kind
  for kind in issue pr; do
    gh "$kind" list -R "$REPO" --assignee @me --state open --search "updated:>=$SINCE_Z" \
      --json number,title,url,updatedAt \
      --jq ".[] | {id: (\"assigned-$kind-\\(.number)\"), title: (\"Assigned to you: #\\(.number) \\(.title)\"), url: .url, at: .updatedAt, text: \"open $kind assigned to you\"}"
  done
}

activity_on_my_pull_requests() {
  local number numbers
  numbers=$(gh pr list -R "$REPO" --author @me --state open --json number --jq '.[].number') || fail "listing your pull requests failed"
  for number in $numbers; do
    gh pr view "$number" -R "$REPO" --json url,reviews,comments --jq ".url as \$url | \
      (.reviews[]? | select(.submittedAt >= \"$SINCE_Z\") | {id: (\"review-\" + .id), title: (\"Review \" + .state + \" by \" + .author.login + \" on PR #$number\"), url: \$url, at: .submittedAt, text: .body}), \
      (.comments[]? | select(.createdAt >= \"$SINCE_Z\") | {id: (\"comment-\" + .id), title: (\"Comment by \" + .author.login + \" on PR #$number\"), url: (.url // \$url), at: .createdAt, text: .body})"
  done
}

branch_commits() {
  gh api "repos/$REPO/commits?sha=$BRANCH&since=$SINCE_Z&per_page=30" \
    --jq '.[] | {id: ("commit-" + .sha[0:12]), title: (.commit.message | split("\n")[0]), url: .html_url, at: .commit.committer.date, text: ("by " + (.author.login // .commit.author.name))}'
}

branch_checks() {
  gh api "repos/$REPO/commits/$BRANCH/check-runs?per_page=100" --jq "[.check_runs[] | select(.completed_at >= \"$SINCE_Z\" and (.conclusion == \"failure\" or .conclusion == \"timed_out\"))] | if length == 0 then empty else {id: (\"ci-\" + .[0].head_sha[0:12]), title: (\"Failing checks on $BRANCH\"), url: .[0].html_url, at: .[0].completed_at, text: (map(.name) | join(\", \"))} end"
}

merge_state() {  # mergeable merge-state-status -> clean, behind, conflict or unknown
  case "$1:$2" in
    UNKNOWN:*|*:UNKNOWN) echo unknown ;;
    CONFLICTING:*|*:DIRTY) echo conflict ;;
    *:BEHIND) echo behind ;;
    *) echo clean ;;
  esac
}

previous_signature() {  # pull-request-number -> what the last run knew of it
  printf '%s' "$CURSOR" | tr ';' '\n' | sed -n "s/^$1=//p"
}

my_pull_request_states() {
  local number title url base head oid failing mergeable status fork state signature previous now behind text listing
  now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  listing=$(gh pr list -R "$REPO" --author @me --state open \
    --json number,title,url,baseRefName,headRefName,headRefOid,statusCheckRollup,mergeable,mergeStateStatus,isCrossRepository \
    --jq '.[] | [.number, .title, .url, .baseRefName, .headRefName, .headRefOid, ([.statusCheckRollup[]? | select(.conclusion == "FAILURE" or .conclusion == "TIMED_OUT" or .state == "FAILURE" or .state == "ERROR")] | length), .mergeable, .mergeStateStatus, .isCrossRepository] | @tsv') \
    || fail "listing your pull requests failed"
  while IFS=$'\t' read -r number title url base head oid failing mergeable status fork; do
    [ -n "$number" ] || continue
    state=$(merge_state "$mergeable" "$status")
    previous=$(previous_signature "$number")
    if [ "$state" = unknown ]; then  # GitHub is still computing it: say nothing, keep what the last run knew
      [ -z "$previous" ] || NEW_CURSOR="$NEW_CURSOR$number=$previous;"
      continue
    fi
    signature="$state:$failing:${oid:0:12}"
    NEW_CURSOR="$NEW_CURSOR$number=$signature;"
    if [ "$signature" != "$previous" ] && { [ "$state" != clean ] || [ "$failing" != 0 ] || [ -n "$previous" ]; }; then
      text="$title. Merge state against $base: $state. Failing checks: $failing."
      if [ "$state" = behind ] && [ "$fork" != true ]; then  # a fork's branch is not in this repository to compare
        behind=$(gh api "repos/$REPO/compare/$base...$head" --jq .behind_by) || fail "the compare query for PR #$number failed"
        text="$text Behind by $behind commits."
      fi
      emit "state-$number-${oid:0:12}" "Your PR #$number is $state with $base" "$url" "$now" "$text"
    fi
  done <<< "$listing"
}

NEW_CURSOR=
pull_requests
issues
assignments
activity_on_my_pull_requests
branch_commits
branch_checks
my_pull_request_states
printf '{"cursor": "%s"}\n' "$NEW_CURSOR"
