"""Run a release cargo build with workstation paths remapped in the distributed executable.

Usage: build.py <cargo build arguments, such as --release --locked -p sakura-app>
Environment: SAKURA_CARGO selects the cargo executable. Existing compiler flags are preserved.
"""
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def release_environment(environment, home, root):
    """Preserve caller flags and remap the workspace after its enclosing home directory."""
    result = dict(environment)
    if "CARGO_ENCODED_RUSTFLAGS" in result:
        flags = result["CARGO_ENCODED_RUSTFLAGS"].split("\x1f") if result["CARGO_ENCODED_RUSTFLAGS"] else []
    else:
        flags = result.get("RUSTFLAGS", "").split()
    target = Path(result.get("CARGO_TARGET_DIR", root / "target"))
    if not target.is_absolute():
        target = root / target
    flags.extend((f"--remap-path-prefix={home}=/build/home",
                  f"--remap-path-prefix={target}=/build/target", f"--remap-path-prefix={root}=."))
    result["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(flags)
    return result


def main(arguments):
    """Build with the caller's cargo executable and propagate its result."""
    environment = release_environment(os.environ, Path.home(), ROOT)
    command = [environment.get("SAKURA_CARGO", "cargo"), "build", *arguments]
    return subprocess.run(command, cwd=ROOT, env=environment, stdin=subprocess.DEVNULL).returncode


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
