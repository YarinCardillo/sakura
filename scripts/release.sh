#!/bin/bash
# Build local release artifacts for macOS. Publication is deliberately a printed command.
#
# Usage: scripts/release.sh <version>
# Environment:
#   SAKURA_SIGN_IDENTITY    the signing identity (default: Developer ID Application); - forces ad hoc without keychain probes
#   SAKURA_NOTARY_PROFILE   the notarytool keychain profile (default: sakura-notary)
#   SAKURA_APP_BUNDLE       a Sakura.app that is already built (for example the one CI built); it is
#                           copied and signed instead of building here
#   SAKURA_CARGO            the cargo command to run (default: cargo)
#   CARGO_TARGET_DIR        where cargo builds
# The script never reads, prints or creates a credential: it asks codesign and notarytool to use what
# the keychain already holds, and every such call has a deadline, so a keychain prompt nobody answers
# stops the release instead of hanging it. Without the identity or the profile the bundle is signed
# ad hoc, and the output says so.
set -euo pipefail
umask 022
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
if [[ $# != 1 ]]; then
    printf 'Usage: scripts/release.sh <version>\n' >&2
    exit 64
fi
VERSION=$1
REPO=YarinCardillo/sakura
IDENTITY=${SAKURA_SIGN_IDENTITY:-Developer ID Application}
PROFILE=${SAKURA_NOTARY_PROFILE:-sakura-notary}
TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target}
RELEASES="$ROOT/dist/releases"
OUT="$RELEASES/$VERSION"
APP="$OUT/Sakura.app"
ZIP="$OUT/Sakura-$VERSION.zip"
CALL_DEADLINE_SECONDS=120
UPLOAD_DEADLINE_SECONDS=900
NOTARY_WAIT=30m

require_clean_worktree() {
    local status
    status=$(git status --porcelain=v1 --untracked-files=all)
    [[ -z "$status" ]] || { printf 'Release requires a clean worktree, including untracked files.\n' >&2; exit 1; }
}

# A published tag is never moved: a fix gets a new version.
require_unused_tag() {
    local tagged
    tagged=$(git rev-parse -q --verify "refs/tags/v$VERSION^{commit}" || true)
    [[ -z "$tagged" || "$tagged" == "$SOURCE_COMMIT" ]] || {
        printf 'Tag v%s already names another commit. Use a new version.\n' "$VERSION" >&2
        exit 1
    }
}

# Runs a command that may wait for a person or for the network, with a deadline in seconds.
with_deadline() {
    local seconds=$1
    shift
    perl -e 'alarm shift; exec @ARGV' "$seconds" "$@" < /dev/null
}

# Whether the keychain holds the identity and the notary profile answers. Nothing they print is shown.
can_sign_for_distribution() {
    [[ "$IDENTITY" != "-" ]] || return 1
    local identities
    identities=$(with_deadline "$CALL_DEADLINE_SECONDS" security find-identity -v -p codesigning 2> /dev/null) || return 1
    grep -Fq "$IDENTITY" <<< "$identities" || return 1
    with_deadline "$CALL_DEADLINE_SECONDS" xcrun notarytool history --keychain-profile "$PROFILE" \
        > /dev/null 2>&1
}

architecture_of() {
    local archs
    archs=$(lipo -archs "$1")
    case "$archs" in
        *arm64*x86_64* | *x86_64*arm64*) printf 'universal' ;;
        *) printf '%s' "$archs" ;;
    esac
}

build_bundle() {
    if [[ -n ${SAKURA_APP_BUNDLE:-} ]]; then
        printf 'Using the bundle at %s\n' "$SAKURA_APP_BUNDLE"
        ditto "$SAKURA_APP_BUNDLE" "$APP"
        return
    fi
    local universal=0 installed
    installed=$(rustup target list --installed 2> /dev/null || true)
    if grep -q '^aarch64-apple-darwin$' <<< "$installed" && grep -q '^x86_64-apple-darwin$' <<< "$installed"; then
        universal=1
    else
        printf 'Only one Rust target for macOS is installed: the bundle is for this architecture only.\n'
    fi
    SAKURA_UNIVERSAL=$universal scripts/bundle-macos.sh < /dev/null > "$OUT/logs/build.log" 2>&1
    ditto "$TARGET_DIR/bundle/Sakura.app" "$APP"
}

notarize() {
    local artifact=$1 submission
    with_deadline "$UPLOAD_DEADLINE_SECONDS" xcrun notarytool submit "$artifact" --keychain-profile "$PROFILE" \
        --output-format json > "$OUT/logs/notary-submit.json"
    submission=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' "$OUT/logs/notary-submit.json")
    printf 'Waiting for notarization: %s\n' "$submission"
    xcrun notarytool wait "$submission" --keychain-profile "$PROFILE" \
        --timeout "$NOTARY_WAIT" --output-format json < /dev/null > "$OUT/logs/notary.json"
    python3 - "$OUT/logs/notary.json" <<'PY'
import json
import sys
from pathlib import Path
result = json.loads(Path(sys.argv[1]).read_text())
print("Notarization:", result.get("id"), result.get("status"))
if result.get("status") != "Accepted":
    sys.exit("Notarization was not accepted; inspect the submission log before retrying")
PY
}

verify_distributed_app() {
    local bundle=$1
    with_deadline "$CALL_DEADLINE_SECONDS" codesign --verify --deep --strict --verbose=2 "$bundle"
    with_deadline "$CALL_DEADLINE_SECONDS" spctl --assess --type execute --verbose=2 "$bundle"
    with_deadline "$CALL_DEADLINE_SECONDS" xcrun stapler validate "$bundle"
}

sign_for_distribution() {
    python3 scripts/sign_bundle.py "$APP" "$IDENTITY" scripts/release.entitlements
    printf 'Submitting the app for notarization\n'
    ditto -c -k --sequesterRsrc --keepParent "$APP" "$OUT/notarization.zip"
    notarize "$OUT/notarization.zip"
    rm "$OUT/notarization.zip"
    with_deadline "$CALL_DEADLINE_SECONDS" xcrun stapler staple "$APP"
    verify_distributed_app "$APP" > "$OUT/logs/verify-app.log" 2>&1
    printf 'developer-id-notarized\n' > "$OUT/signature.txt"
}

sign_ad_hoc() {
    printf 'No "%s" identity or no working notary profile "%s": the bundle is signed AD HOC.\n' "$IDENTITY" "$PROFILE"
    printf 'Gatekeeper refuses an ad hoc build downloaded on another Mac until the person allows it.\n'
    with_deadline "$CALL_DEADLINE_SECONDS" codesign --force --sign - --timestamp=none "$APP"
    with_deadline "$CALL_DEADLINE_SECONDS" codesign --verify --strict "$APP"
    printf 'ad-hoc\n' > "$OUT/signature.txt"
}

# The command that publishes: it adds to the draft the tag's workflow made, or makes the draft.
write_publish_command() {
    local assets=("$ZIP" "$OUT/SHA256SUMS-macos.txt") create
    create=(gh release create "v$VERSION" --repo "$REPO" --draft --title "Sakura $VERSION"
        --target "$SOURCE_COMMIT" --notes-file "$OUT/release-notes.md")
    if [[ $VERSION == *-* ]]; then create+=(--prerelease); fi
    {
        printf 'if gh release view %q --repo %q > /dev/null 2>&1; then\n' "v$VERSION" "$REPO"
        printf '  [[ %s(gh release view %q --repo %q --json isDraft --jq .isDraft) == true ]] || { printf "Release already published; use a new version.\\n" >&2; exit 1; }\n' '$' "v$VERSION" "$REPO"
        printf '  gh release upload %q --repo %q'  "v$VERSION" "$REPO"
        printf ' %q' "${assets[@]}"
        printf '\nelse\n '
        printf ' %q' "${create[@]}" "${assets[@]}"
        printf '\nfi\n'
    } > "$OUT/publish-draft.sh"
}

require_clean_worktree
SOURCE_COMMIT=$(git rev-parse HEAD)
require_unused_tag
python3 scripts/release_metadata.py version Cargo.toml "$VERSION" > /dev/null
NOTES=$(python3 scripts/release_metadata.py notes CHANGELOG.md "$VERSION")
[[ ! -e "$OUT" ]] || { printf 'Output already exists: %s. Keep it and use a new version, or move the failed attempt.\n' "$OUT" >&2; exit 1; }
for tool in codesign ditto shasum git perl python3 lipo; do
    command -v "$tool" > /dev/null
done
mkdir -p "$OUT/logs"
exec > >(tee "$OUT/logs/pipeline.log") 2>&1
trap 'printf "Release stopped at line %s. Logs: %s/logs\n" "$LINENO" "$OUT" >&2' ERR
printf '%s\n' "$NOTES" > "$OUT/release-notes.md"
printf '%s\n' "$SOURCE_COMMIT" > "$OUT/source-commit.txt"

printf 'Building Sakura %s\n' "$VERSION"
build_bundle
require_clean_worktree
[[ $(git rev-parse HEAD) == "$SOURCE_COMMIT" ]] || { printf 'Source commit changed during the build.\n' >&2; exit 1; }
python3 scripts/release_metadata.py bundle "$APP/Contents/Info.plist" "$VERSION"
ARCHITECTURE=$(architecture_of "$APP/Contents/MacOS/sakura")
printf 'Architecture: %s\n' "$ARCHITECTURE"

if can_sign_for_distribution; then
    sign_for_distribution
else
    sign_ad_hoc
fi
with_deadline "$CALL_DEADLINE_SECONDS" codesign -d --verbose=4 "$APP" 2> "$OUT/logs/codesign-details.log"

ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"
# Verify exactly what people will extract, including the stapled ticket when there is one.
mkdir "$OUT/zip-check"
ditto -x -k "$ZIP" "$OUT/zip-check"
if [[ $(cat "$OUT/signature.txt") == developer-id-notarized ]]; then
    verify_distributed_app "$OUT/zip-check/Sakura.app" > "$OUT/logs/verify-zip.log" 2>&1
else
    with_deadline "$CALL_DEADLINE_SECONDS" codesign --verify --strict "$OUT/zip-check/Sakura.app"
fi
rm -r "$OUT/zip-check"

python3 scripts/write_cask.py "$VERSION" "$ZIP" "$OUT/sakura.rb" "$ARCHITECTURE"
(cd "$OUT" && shasum -a 256 "${ZIP##*/}") > "$OUT/SHA256SUMS-macos.txt"
write_publish_command
mkdir -p Casks
cp "$OUT/sakura.rb" Casks/sakura.rb
printf 'Local release ready (%s). Review the artifacts in %s and the updated Casks/sakura.rb.\n' \
    "$(cat "$OUT/signature.txt")" "$OUT"
printf 'Publication command (NOT executed):\n'
cat "$OUT/publish-draft.sh"
