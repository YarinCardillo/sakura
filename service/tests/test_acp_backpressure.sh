#!/bin/bash
# A full reply queue must not hide that the client closed its side of a conversation.
. "$(dirname "$0")/lib.sh"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/box-test.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
cat >"$TMP/agent" <<'AGENT'
#!/usr/bin/perl
$| = 1;
while (1) { print "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"fs/read_text_file\"}\n"; }
AGENT
chmod +x "$TMP/agent"
assert_status "client closure starts its deadline even with a full refusal queue" 0 \
  python3 - "$BOX_ROOT/lib/acp_proxy.pl" "$TMP" <<'PY'
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

folder = Path(sys.argv[2])
errors = (folder / 'relay.err').open('wb')
process = subprocess.Popen(
    ['perl', sys.argv[1], '1024', '1', '30', str(folder / 'report'), str(folder / 'agent')],
    stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=errors, start_new_session=True)
try:
    time.sleep(1)
    started = time.monotonic()
    process.stdin.close()
    code = process.wait(timeout=6)
    assert code == 66, code
    assert time.monotonic() - started < 5
    assert b'did not end within 1 s' in (folder / 'relay.err').read_bytes()
finally:
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=5)
    errors.close()
PY
finish
