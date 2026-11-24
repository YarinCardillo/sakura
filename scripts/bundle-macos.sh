#!/bin/bash
# Builds the release binary and assembles Sakura.app in "$CARGO_TARGET_DIR/bundle" (default: target/bundle):
# the binary, the icon, the service in Resources/service (laid out by scripts/package.py), the pinned
# Sparkle framework (scripts/embed-sparkle.sh), the Leeway helper and tmux in Contents/Helpers (when given)
# and an ad hoc signature. scripts/release.sh signs it again
# with a Developer ID when one is available.
#
# Usage: scripts/bundle-macos.sh
# Environment:
#   CARGO_TARGET_DIR   where cargo builds
#   SAKURA_CARGO       the cargo command to run (default: cargo)
#   SAKURA_LEEWAY_HELPER
#                      a pinned release build of the leeway command; it is copied to Contents/Helpers/leeway
#                      (with the file `<path>.commit`, when there is one, as Contents/Resources/leeway.commit) and signed
#                      before the app is, so the app can name it to the engine. Without it the bundle has no
#                      helper and the engine looks for Leeway on its own.
#   SAKURA_TMUX_HELPER
#                      a tmux build; it is copied to Contents/Helpers/tmux and so lanes run on a Mac without Homebrew.
#                      Every library it links outside the system (libevent, ncurses, utf8proc) is copied to
#                      Contents/Frameworks and the install names are rewritten to point there; the ncurses copy looks
#                      for terminal descriptions in /usr/share/terminfo, which every Mac has. The libraries and tmux are
#                      signed with the hardened runtime before the app is. `tmux -V` is recorded as
#                      Contents/Resources/tmux.version and the file `<path>.notices` (the licences of tmux and of those
#                      libraries) as Contents/Resources/tmux.notices. Without it the bundle has no tmux and the engine
#                      looks for one on the PATH. A universal bundle needs a tmux with both architectures.
#   SAKURA_UNIVERSAL   1 builds arm64 and x86_64 and joins them (both Rust targets must be installed)
#   SAKURA_UPDATE_FEED_URL, SAKURA_UPDATE_PUBLIC_KEY
#                      together, the update feed and public key written into the bundle (docs/release.md);
#                      without them the app says updates are not set up in this build
set -euo pipefail

readonly BUNDLE_ID="dev.yarin.sakura"
readonly APP_NAME="Sakura"
readonly BINARY_NAME="sakura"
readonly PACKAGE="sakura-app"
readonly MINIMUM_MACOS="12.0"
readonly TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
readonly SIGN_TIMEOUT_SECONDS=120
readonly HELPER_NAME="leeway"
readonly TMUX_NAME="tmux"
readonly SYSTEM_TERMINFO="/usr/share/terminfo"

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

# Copies the pinned Leeway build to Contents/Helpers and signs it, so that the app's own signature, made
# after it, seals it. A given path that is not an executable file stops the bundle.
embed_helper() {
  local source="${SAKURA_LEEWAY_HELPER:-}" helpers="$app/Contents/Helpers"
  if [ -z "$source" ]; then
    echo "bundle: SAKURA_LEEWAY_HELPER is not set; the bundle has no Leeway helper" >&2
    return
  fi
  [ -f "$source" ] && [ -x "$source" ] || { echo "bundle: $source is not an executable file" >&2; exit 1; }
  mkdir -p "$helpers"
  cp "$source" "$helpers/$HELPER_NAME"
  # Resources is sealed by the app's signature; a plain file in Helpers is not, and codesign refuses it.
  if [ -f "$source.commit" ]; then cp "$source.commit" "$app/Contents/Resources/$HELPER_NAME.commit"; fi
  with_deadline codesign --force --sign - --options runtime --timestamp=none "$helpers/$HELPER_NAME"
  with_deadline codesign --verify --strict "$helpers/$HELPER_NAME"
}

# The libraries `image` links outside the system, one install name per line. A relative name that is not
# inside the bundle is refused: nothing here can tell where it would be found.
foreign_libraries() {
  local library
  otool -L "$1" | tail -n +2 | awk '{print $1}' | while read -r library; do
    case "$library" in
      /usr/lib/*|/System/*|@executable_path/*|@loader_path/*) ;;
      /*) echo "$library" ;;
      *) echo "bundle: $1 links $library, which cannot be placed in the bundle" >&2; exit 1 ;;
    esac
  done
}

# An ncurses library looks for terminal descriptions in the folder it was built for (a Homebrew keg). Points the copy
# at the system's own folder by rewriting that string in place, padded with NULs to the same length.
point_terminfo_at_system() {
  python3 -I - "$1" "$SYSTEM_TERMINFO" <<'PY'
import re, sys
path, system = sys.argv[1], sys.argv[2].encode()
data = open(path, "rb").read()
compiled = re.compile(rb"(?<=\x00)/[^\x00]*/share/terminfo(?=\x00)")
found = compiled.findall(data)
if len(found) != 1 or len(found[0]) < len(system):
    sys.exit("bundle: %s: expected one terminfo folder to rewrite, found %r" % (path, found))
data = compiled.sub(lambda match: system + b"\x00" * (len(match.group()) - len(system)), data)
open(path, "wb").write(data)
PY
}

# Copies each library `image` links outside the system into Contents/Frameworks, points `image` at the copy through
# `prefix` (the folder of the copies as `image` sees it), and does the same for each copy.
bundle_libraries() {
  local image="$1" prefix="$2" library name copy
  while read -r library; do
    name="$(basename "$library")"
    copy="$app/Contents/Frameworks/$name"
    if [ ! -f "$copy" ]; then
      cp -L "$library" "$copy"
      chmod u+w "$copy"
      install_name_tool -id "@loader_path/$name" "$copy"
      case "$name" in libncurses*|libcurses*|libtinfo*) point_terminfo_at_system "$copy" ;; esac
      bundle_libraries "$copy" "@loader_path"
    fi
    install_name_tool -change "$library" "$prefix/$name" "$image"
  done < <(foreign_libraries "$image")
}

# The hardened runtime refuses a library that does not share the program's Team ID, and an ad hoc signature has none:
# an ad hoc tmux needs the one entitlement that lifts that, and only here. scripts/release.sh signs tmux and its libraries
# again with one Developer ID and no entitlement, so a distributed build keeps the check.
sign_tmux_ad_hoc() {
  local entitlements
  entitlements="$(mktemp "${TMPDIR:-/tmp}/sakura-tmux-entitlements.XXXXXX")"
  cat > "$entitlements" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict><key>com.apple.security.cs.disable-library-validation</key><true/></dict>
</plist>
PLIST
  with_deadline codesign --force --sign - --options runtime --timestamp=none --entitlements "$entitlements" "$1"
  rm -f "$entitlements"
}

# Copies the given tmux to Contents/Helpers with its libraries, and signs the libraries and then tmux with the hardened
# runtime, so that the app's own signature, made after them, seals them. A given path that is not an executable
# file, or a tmux that cannot run on every architecture of the bundle, stops the bundle.
embed_tmux() {
  local source="${SAKURA_TMUX_HELPER:-}" helpers="$app/Contents/Helpers" frameworks="$app/Contents/Frameworks" library version
  if [ -z "$source" ]; then
    echo "bundle: SAKURA_TMUX_HELPER is not set; the bundle has no tmux" >&2
    return
  fi
  [ -f "$source" ] && [ -x "$source" ] || { echo "bundle: $source is not an executable file" >&2; exit 1; }
  if [ "${SAKURA_UNIVERSAL:-0}" = 1 ] && ! { lipo -archs "$source" | grep -q arm64 && lipo -archs "$source" | grep -q x86_64; }; then
    echo "bundle: $source does not have both architectures of a universal bundle" >&2
    exit 1
  fi
  [ -f "$source.notices" ] || { echo "bundle: $source.notices (the licences) does not exist" >&2; exit 1; }
  mkdir -p "$helpers" "$frameworks"
  cp "$source" "$helpers/$TMUX_NAME"
  chmod u+w "$helpers/$TMUX_NAME"
  bundle_libraries "$helpers/$TMUX_NAME" "@executable_path/../Frameworks"
  for library in "$frameworks"/*.dylib; do
    with_deadline codesign --force --sign - --options runtime --timestamp=none "$library"
    with_deadline codesign --verify --strict "$library"
  done
  sign_tmux_ad_hoc "$helpers/$TMUX_NAME"
  with_deadline codesign --verify --strict "$helpers/$TMUX_NAME"
  version="$("$helpers/$TMUX_NAME" -V)"
  [ -n "$version" ] || { echo "bundle: the bundled tmux printed no version" >&2; exit 1; }
  echo "$version" > "$app/Contents/Resources/$TMUX_NAME.version"
  cp "$source.notices" "$app/Contents/Resources/$TMUX_NAME.notices"
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
  local version sparkle
  version="$(workspace_version)"
  [ -n "$version" ] || { echo "bundle: no version in Cargo.toml" >&2; exit 1; }
  sparkle="$("$root/scripts/fetch-sparkle.sh")"
  export SAKURA_SPARKLE_FRAMEWORK_DIR="$sparkle"
  rm -rf "$app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  build_binary "$app/Contents/MacOS/$BINARY_NAME"
  cp "$root/assets/icons/$APP_NAME.icns" "$app/Contents/Resources/$APP_NAME.icns"
  python3 "$root/scripts/package.py" service "$app/Contents/Resources/service" >/dev/null
  python3 "$root/scripts/package.py" documents "$app/Contents/Resources" >/dev/null
  write_info_plist "$version"
  "$root/scripts/embed-sparkle.sh" "$app"
  "$root/scripts/sign-sparkle.sh" "$app" -
  embed_helper
  embed_tmux
  with_deadline codesign --force --sign - --timestamp=none "$app"
  with_deadline codesign --verify --strict "$app"
  echo "$app"
}

main "$@"
