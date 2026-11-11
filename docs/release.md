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

Open the signed app from Finder before publishing, and check that About shows the version.

`publish-draft.sh` holds the publication command (it adds the zip to the draft CI made, or makes the
draft). It is printed, not run. Publishing the draft on GitHub is the last step.

The script refuses to overwrite a version's existing output: keep a failed attempt for its logs, or
move it aside. Once anyone has downloaded a build, use a new version.
