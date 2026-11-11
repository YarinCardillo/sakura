#!/bin/bash
# Builds the release binary and assembles Sakura.app in "$CARGO_TARGET_DIR/bundle" (default: target/bundle):
# the binary, the icon, the service in Resources/service (laid out by scripts/package.py), and an ad hoc
# signature. scripts/release.sh signs it again with a Developer ID when one is available.
#
# Usage: scripts/bundle-macos.sh
# Environment:
#   CARGO_TARGET_DIR   where cargo builds
#   SAKURA_CARGO       the cargo command to run (default: cargo)
#   SAKURA_UNIVERSAL   1 builds arm64 and x86_64 and joins them (both Rust targets must be installed)
set -euo pipefail

readonly BUNDLE_ID="dev.yarin.sakura"
readonly APP_NAME="Sakura"
readonly BINARY_NAME="sakura"
readonly PACKAGE="sakura-app"
readonly MINIMUM_MACOS="11.0"
readonly TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
readonly SIGN_TIMEOUT_SECONDS=120

root="$(cd "$(dirname "$0")/.." && pwd)"
target_dir="${CARGO_TARGET_DIR:-$root/target}"
app="$target_dir/bundle/$APP_NAME.app"

workspace_version() {
  sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1
}

# Runs a command that may wait for a person (a keychain prompt) with a deadline.
with_deadline() {
  perl -e 'alarm shift; exec @ARGV' "$SIGN_TIMEOUT_SECONDS" "$@"
}

build_binary() {
  local destination="$1" target
  if [ "${SAKURA_UNIVERSAL:-0}" != 1 ]; then
    (cd "$root" && python3 "$root/scripts/build.py" --release --locked -p "$PACKAGE")
    cp "$target_dir/release/$BINARY_NAME" "$destination"
    return
  fi
  for target in "${TARGETS[@]}"; do
    (cd "$root" && python3 "$root/scripts/build.py" --release --locked -p "$PACKAGE" --target "$target")
  done
  lipo -create "$target_dir/${TARGETS[0]}/release/$BINARY_NAME" "$target_dir/${TARGETS[1]}/release/$BINARY_NAME" \
    -output "$destination"
}

write_info_plist() {
  local version="$1"
  cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleLocalizations</key>
  <array><string>en</string><string>it</string></array>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundleExecutable</key><string>$BINARY_NAME</string>
  <key>CFBundleIconFile</key><string>$APP_NAME</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>LSMinimumSystemVersion</key><string>$MINIMUM_MACOS</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST
}

main() {
  local version
  version="$(workspace_version)"
  [ -n "$version" ] || { echo "bundle: no version in Cargo.toml" >&2; exit 1; }
  rm -rf "$app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  build_binary "$app/Contents/MacOS/$BINARY_NAME"
  cp "$root/assets/icons/$APP_NAME.icns" "$app/Contents/Resources/$APP_NAME.icns"
  python3 "$root/scripts/package.py" service "$app/Contents/Resources/service" >/dev/null
  python3 "$root/scripts/package.py" documents "$app/Contents/Resources" >/dev/null
  write_info_plist "$version"
  with_deadline codesign --force --sign - --timestamp=none "$app"
  with_deadline codesign --verify --strict "$app"
  echo "$app"
}

main "$@"
