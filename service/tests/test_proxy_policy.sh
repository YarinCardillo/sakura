#!/bin/bash
# The proxy lets through one thing: a tunnel (CONNECT) to port 443 of a name on the allow list.
# ConnectPort bounds the port of a tunnel only; an ordinary HTTP request through the proxy
# ("GET http://name:22/") names its own port. So the proxy filters the whole request target and
# not the host alone, and every line of the allow list is written for the target of a tunnel,
# "<name>:443": the target of an ordinary request begins with "http://" and matches no line.
# This test reads the two files and applies the lines the way tinyproxy does (extended regular
# expressions, case ignored, searched in the target). The proxy itself is tested live by
# tests/confinement.sh.
. "$(dirname "$0")/lib.sh"
readonly CONF="$BOX_ROOT/proxy/tinyproxy.conf" ALLOW="$BOX_ROOT/proxy/allow.txt"
FAILURES=0
setting() {
  sed -n "s/^$1[[:space:]]\{1,\}//p" "$CONF"
}
allowed() {
  printf '%s\n' "$1" | grep -E -i -q -f "$ALLOW" && echo yes || echo no
}

echo " the configuration"
assert_eq "the whole request target is filtered, not the host alone" "On" "$(setting FilterURLs)"
assert_eq "what matches no line is refused" "Yes" "$(setting FilterDefaultDeny)"
assert_eq "the lines are extended regular expressions" "ere" "$(setting FilterType)"
assert_eq "tunnels go to port 443 only" "443" "$(setting ConnectPort | tr '\n' ' ' | sed 's/ $//')"

echo " every line of the allow list is one name with port 443, anchored at both ends"
assert_eq "no line of another form" "0" "$(grep -v -c -E '^\^[a-z0-9\\.-]+:443\$$' "$ALLOW")"
assert_eq "(the list is not empty)" "1" "$([ "$(wc -l <"$ALLOW")" -ge 1 ] && echo 1 || echo 0)"

echo " tunnels to port 443 of a name on the list"
for target in registry.npmjs.org:443 github.com:443 API.ANTHROPIC.COM:443; do
  assert_eq "$target" "yes" "$(allowed "$target")"
done

echo " everything else"
for target in "http://github.com:22/" "http://github.com/" "http://github.com:443/" "http://registry.npmjs.org/" \
  "github.com:22" "github.com" "github.com:4430" "github.com:443:22" "example.com:443" "github.com.example.com:443" \
  "xgithub.com:443" "http://example.com/github.com:443" "http://example.com/?github.com:443" "1.1.1.1:443" "[::1]:443"; do
  assert_eq "$target" "no" "$(allowed "$target")"
done
finish
