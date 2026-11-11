"""Validate release inputs without accessing credentials: the version, the notes, the built bundle."""
import argparse
import json
from pathlib import Path
import plistlib
import re
import subprocess
import sys

VERSION_PATTERN = r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-(alpha|beta|rc)\.([1-9][0-9]*))?"


def read_version(manifest, requested):
    """Return the requested version when the workspace manifest declares exactly it."""
    if not re.fullmatch(VERSION_PATTERN, requested):
        raise ValueError("Use MAJOR.MINOR.PATCH or MAJOR.MINOR.PATCH-{alpha,beta,rc}.N")
    section = re.search(r"^\[workspace\.package\]\n(.*?)(?=^\[|\Z)", manifest, re.MULTILINE | re.DOTALL)
    declared = re.search(r'^version\s*=\s*"([^"]+)"', section.group(1), re.MULTILINE) if section else None
    if declared is None:
        raise ValueError("Cargo.toml declares no workspace version")
    if declared.group(1) != requested:
        raise ValueError(f"Requested version {requested} differs from Cargo.toml ({declared.group(1)})")
    return requested


def release_notes(changelog, version):
    """Return the nonempty, exact version section, excluding other releases."""
    heading = rf"^## (?:\[{re.escape(version)}\]|{re.escape(version)})(?: - [0-9-]+)?\s*\n"
    match = re.search(heading, changelog, re.MULTILINE)
    if match is None:
        raise ValueError(f"CHANGELOG.md has no section for {version}")
    section = re.split(r"^## ", changelog[match.end():], maxsplit=1, flags=re.MULTILINE)[0].strip()
    if not section:
        raise ValueError(f"CHANGELOG.md section for {version} is empty")
    return section + "\n"


def check_bundle(info_plist, version):
    """Reject a built bundle that reports another version than the one being released."""
    with open(info_plist, "rb") as stream:
        info = plistlib.load(stream)
    reported = (info.get("CFBundleShortVersionString"), info.get("CFBundleVersion"))
    if reported != (version, version):
        raise ValueError(f"The built bundle reports {reported}, not {version}")
    return f"Bundle version: {version}"


def require_draft(repository, tag):
    """Refuse uploads to published releases, including workflow reruns on an existing tag."""
    response = subprocess.run(["gh", "release", "view", tag, "--repo", repository, "--json", "isDraft"],
                              check=True, capture_output=True, text=True, timeout=30,
                              stdin=subprocess.DEVNULL)
    if json.loads(response.stdout).get("isDraft") is not True:
        raise ValueError(f"Release {tag} is published or its draft state is unknown; use a new version")
    return f"Release {tag} is a draft"


def main():
    """Expose release validation and changelog extraction to release.sh."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["version", "notes", "bundle", "draft"])
    parser.add_argument("inputs", nargs=2)
    args = parser.parse_args()
    source, version = args.inputs
    if args.action == "draft":
        print(require_draft(source, version))
    elif args.action == "version":
        print(read_version(Path(source).read_text(), version))
    elif args.action == "notes":
        print(release_notes(Path(source).read_text(), version), end="")
    else:
        print(check_bundle(source, version))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, plistlib.InvalidFileException, subprocess.SubprocessError) as error:
        sys.exit(str(error))
