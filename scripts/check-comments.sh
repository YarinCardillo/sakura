#!/bin/sh
set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
if [ "$#" -eq 0 ]; then
    cd "$script_dir/.."
    set -- crates/sakura-app crates/sakura-core crates/sakura-acp crates/sakura-terminal
fi
exec python3 "$script_dir/check-comments.py" "$@"
