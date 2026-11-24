#!/bin/bash
# Embed the pinned Sparkle.framework and its MIT license in an assembled app, and write the update keys
# into its Info.plist when SAKURA_UPDATE_FEED_URL and SAKURA_UPDATE_PUBLIC_KEY are both set (without
# them the app says "Updates are not set up in this build"). Run before any signing.
#
set -euo pipefail
app=$1
root="$(cd "$(dirname "$0")/.." && pwd)"
sparkle=${2:-}
if [[ -z "$sparkle" ]]; then
  sparkle="$(bash "$root/scripts/fetch-sparkle.sh")"
fi

thin_arm64() {
  local file architectures
  while IFS= read -r -d '' file; do
    [[ -x "$file" ]] || continue
    architectures="$(lipo -archs "$file" 2>/dev/null)" || continue
    [[ "$architectures" == *" "* ]] || continue
    lipo -thin arm64 "$file" -output "$file.thin"
    chmod "$(stat -f '%Lp' "$file")" "$file.thin"
    mv "$file.thin" "$file"
  done < <(find "$1" -type f -print0)
}

framework="$app/Contents/Frameworks/Sparkle.framework"
mkdir -p "$app/Contents/Frameworks" "$app/Contents/Resources"
rm -rf "$framework"
ditto "$sparkle/Sparkle.framework" "$framework"
if [[ "${3:-}" == arm64 ]]; then
  thin_arm64 "$framework"
fi
cp "$sparkle/LICENSE" "$app/Contents/Resources/Sparkle-LICENSE"
python3 "$root/scripts/update-metadata.py" "$app/Contents/Info.plist"
