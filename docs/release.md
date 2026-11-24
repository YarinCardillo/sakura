# Releasing Sakura

Nothing in the pipeline pushes, creates a repository or publishes a release: publication is a command
the script prints and a person runs.

## Inputs

- The version is the workspace version in `Cargo.toml` (`[workspace.package]`); the lock file follows
  it (`cargo check`). The app's bundle and its About window read it from the build.
- `CHANGELOG.md` has a section `## <version>` with the notes.
- Every release has a new version. A tag that was published is never moved or reused: a fix is a new
  version. The script refuses a version whose tag already names another commit. Uploads refuse a
  published release, including a workflow rerun on its existing tag.

## What CI does on a tag

`git tag v<version> && git push origin v<version>` starts `.github/workflows/release.yml`: it checks
that the tag, the manifest and the changelog agree, then builds on GitHub's runners and uploads to a
**draft** release:

- `Sakura-<version>-macos-unsigned.zip`: the bundle for both Mac architectures, signed ad hoc;
- `sakura-<version>-linux-x86_64.tar.gz`;
- `Sakura-<version>-windows-x86_64.zip`;
- `SHA256SUMS`.

It uses only the workflow token. CI signs the macOS bundle ad hoc; Developer ID signing stays local.
Each archive carries `THIRD_PARTY_NOTICES` (the dependency inventory) and `THIRD_PARTY_LICENSES`
(the original upstream copyright and licence texts, generated from the installed dependency trees).

## The Leeway helper

Sakura reads accounts, limits and pace advice from the `leeway` command. A Mac bundle carries a pinned release
build of it in `Contents/Helpers/leeway`, and the app tells the engine where it is through `SAKURA_LEEWAY`. Set
`SAKURA_LEEWAY_HELPER` to the pinned build before `scripts/bundle-macos.sh` or `scripts/release.sh` runs. If a file
`<path>.commit` sits beside it, the bundle keeps it as `Contents/Resources/leeway.commit`. The helper is signed before
the app is. The unsigned zip that CI builds has no helper, because CI has no pinned build to copy: that bundle finds
Leeway on its own, and the engine says that usage is unavailable when it cannot.

## The signed Mac build, on a Mac

```sh
scripts/release.sh <version>
```

Requires a clean worktree, including untracked files. It builds the bundle (for both architectures
when both Rust targets are installed, otherwise for this Mac's and it says so), checks that the bundle
reports the version, and then:

- when the keychain holds a `Developer ID Application` identity and a working notarytool profile
  (`sakura-notary`), it signs every binary with the hardened runtime, has Apple notarise the app,
  staples the ticket and verifies the result as Gatekeeper will;
- otherwise it signs ad hoc and says so in its output and in `signature.txt`. An ad hoc build is
  refused by Gatekeeper on other Macs until its owner allows it.

`SAKURA_SIGN_IDENTITY` and `SAKURA_NOTARY_PROFILE` name another identity or profile.
`SAKURA_APP_BUNDLE=<path to Sakura.app>` signs a bundle that is already built, for example the one
from CI's zip, instead of building here. The script never reads, prints or creates a credential, and
every call that could wait for a keychain prompt has a deadline.

The profile is created once, by you, with `xcrun notarytool store-credentials sakura-notary`.

The output is in `dist/releases/<version>/`: `Sakura-<version>.zip`, `SHA256SUMS-macos.txt`,
`release-notes.md`, `sakura.rb`, `source-commit.txt`, `signature.txt`, the logs and
`publish-draft.sh`. `Casks/sakura.rb` is updated with the version and the checksum of the final zip:
review it, commit it, and copy it to the Homebrew tap.

## Automatic updates on the Mac

The Mac app checks for updates with [Sparkle](https://sparkle-project.org), pinned by version and
SHA-256 in `scripts/fetch-sparkle.sh` and downloaded into the ignored `build/` folder. The bundle
embeds the framework with its licence, and `scripts/sign-sparkle.sh` signs it inside out (the
Downloader keeps its own entitlements); `scripts/sign_bundle.py` leaves everything inside
`Sparkle.framework` alone. A bundle that was built without a feed and a key says in the app that
updates are not set up in this build.

The feed and the public key come from the environment, together, when the bundle is built (by
`scripts/bundle-macos.sh`, so also by `scripts/release.sh` when it builds):

```sh
export SAKURA_UPDATE_FEED_URL=https://github.com/YarinCardillo/sakura/releases/latest/download/appcast.xml
export SAKURA_UPDATE_PUBLIC_KEY=1mV4WdBdvlmhQEaT3nRXxlkHZN2njtZ1ObiY2oHaYHE=
```

Both values are public. The private half of the key stays in the keychain of the Mac that releases,
under the Sparkle account `sakura`; nothing in this repository creates, reads or prints it.

To publish the feed with a release, also set `SAKURA_SPARKLE_ACCOUNT=sakura`. After the zip is
verified, and only for a notarised build, `scripts/release.sh` signs the zip with that account and
writes `appcast.xml` next to it, adds it to `SHA256SUMS-macos.txt` and to the upload in
`publish-draft.sh`. Without the account the output says the feed was skipped. A build signed ad hoc
never gets a feed, and a bundle without the feed and key is refused when the account is set.

The feed keeps every earlier release: `dist/releases/appcast.xml` holds the feed of the last release
and the next one is merged into it (`SAKURA_PREVIOUS_APPCAST` names another file). The first release
that has a feed has no earlier one: set `SAKURA_FIRST_UPDATE_RELEASE=1` for it. Each version can
appear in the feed once.

Open the signed app from Finder before publishing, and check that About shows the version.

`publish-draft.sh` holds the publication command (it adds the zip to the draft CI made, or makes the
draft). It is printed, not run. Publishing the draft on GitHub is the last step.

The script refuses to overwrite a version's existing output: keep a failed attempt for its logs, or
move it aside. Once anyone has downloaded a build, use a new version.
