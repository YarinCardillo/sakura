"""Generate a reviewable Homebrew cask pinned to the final ZIP checksum."""
import hashlib
from pathlib import Path
import sys

ARCH_LINES = {
    "arm64": "  depends_on arch: :arm64\n",
    "x86_64": "  depends_on arch: :intel\n",
    "universal": "",
}


def checksum(archive):
    """The SHA-256 of the archive, read in pieces."""
    digest = hashlib.sha256()
    with archive.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_cask(version, archive, destination, architecture):
    """Write cask metadata for a verified release archive built for `architecture`."""
    if architecture not in ARCH_LINES:
        raise ValueError(f"Unknown architecture {architecture}: use arm64, x86_64 or universal")
    destination.write_text(f'''cask "sakura" do
  version "{version}"
  sha256 "{checksum(archive)}"

  url "https://github.com/YarinCardillo/sakura/releases/download/v#{{version}}/Sakura-#{{version}}.zip"
  name "Sakura"
  desc "Desk for watching and steering coding agents in confined workspaces"
  homepage "https://github.com/YarinCardillo/sakura"

  depends_on macos: :big_sur
{ARCH_LINES[architecture]}
  app "Sakura.app"
end
''')


if __name__ == "__main__":
    try:
        write_cask(sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3]), sys.argv[4])
    except (ValueError, OSError) as error:
        sys.exit(str(error))
