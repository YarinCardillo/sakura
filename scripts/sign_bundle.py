"""Sign every Mach-O image and code bundle inside out, without deep signing. Every call has a deadline."""
from pathlib import Path
import plistlib
import subprocess
import sys

MACHO_MAGICS = {bytes.fromhex(value) for value in (
    "feedface", "cefaedfe", "feedfacf", "cffaedfe", "cafebabe", "bebafeca", "cafebabf", "bfbafeca")}
CODE_CONTAINERS = {".app", ".xpc", ".framework", ".bundle"}
SIGN_TIMEOUT_SECONDS = 120


def is_macho(path):
    """Identify executable images by their Mach-O header, not by their name."""
    if path.is_symlink() or not path.is_file():
        return False
    with path.open("rb") as stream:
        return stream.read(4) in MACHO_MAGICS


def bundle_executable(bundle):
    """Find the executable signed through its owning code container."""
    if bundle.suffix == ".framework":
        info, base = bundle / "Resources/Info.plist", bundle
    else:
        info, base = bundle / "Contents/Info.plist", bundle / "Contents/MacOS"
    with info.open("rb") as stream:
        name = plistlib.load(stream)["CFBundleExecutable"]
    return (base / name).resolve()


def signing_targets(app):
    """Return inside-out targets; each container signs its own main executable."""
    paths = [path for path in app.rglob("*") if not path.is_symlink()]
    containers = [path for path in paths if path.is_dir() and path.suffix in CODE_CONTAINERS]
    owned = {bundle_executable(bundle) for bundle in containers + [app]}
    images = [path for path in paths if is_macho(path) and path.resolve() not in owned]
    return sorted(images + containers, key=lambda path: len(path.parts), reverse=True) + [app]


def sign_bundle(app, identity, entitlements):
    """Sign inside out with the hardened runtime and a secure timestamp, then verify the whole app."""
    if not any(is_macho(path) for path in app.rglob("*")):
        raise ValueError("App contains no Mach-O images")
    for path in signing_targets(app):
        subprocess.run(["codesign", "--force", "--sign", identity, "--options", "runtime", "--timestamp",
                        "--entitlements", str(entitlements), str(path)],
                       check=True, timeout=SIGN_TIMEOUT_SECONDS, stdin=subprocess.DEVNULL)
    subprocess.run(["codesign", "--verify", "--deep", "--strict", "--verbose=2", str(app)],
                   check=True, timeout=SIGN_TIMEOUT_SECONDS, stdin=subprocess.DEVNULL)


if __name__ == "__main__":
    try:
        sign_bundle(Path(sys.argv[1]), sys.argv[2], Path(sys.argv[3]))
    except subprocess.TimeoutExpired as error:
        sys.exit(f"Signing did not finish within {SIGN_TIMEOUT_SECONDS} s (a keychain prompt?): {error.cmd[0]}")
    except (subprocess.CalledProcessError, ValueError, OSError) as error:
        sys.exit(f"Signing failed: {error}")
