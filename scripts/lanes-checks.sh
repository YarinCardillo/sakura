#!/bin/sh
set -u

cd "$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)" || exit 1

status=0
run() {
    printf '\n$ %s\n' "$*"
    "$@" || status=1
}

run env PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts/tests -p test_check_comments.py
run scripts/check-comments.sh
run cargo test --workspace
run cargo clippy --workspace --all-targets -- -D warnings
run cargo test --workspace --features sakura-app/boxes
run cargo clippy --workspace --all-targets --features sakura-app/boxes -- -D warnings
run cargo test --workspace --features sakura-app/snapshot
run cargo fmt --all --check
exit "$status"
