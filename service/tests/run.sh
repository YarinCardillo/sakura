#!/bin/bash
# Runs every tests/test_*.sh in its own process. Needs no Docker (a fake docker is used).
cd "$(dirname "$0")" || exit 1
status=0
for test_file in test_*.sh; do
  echo "$test_file"
  bash "$test_file" || status=1
done
exit "$status"
