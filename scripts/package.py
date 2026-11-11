"""Lay out what a built Sakura carries, and pack it for Linux or Windows.

    package.py documents <folder>                           the legal documents and README
    package.py service <folder>                             the service as the app carries it, into <folder>
    package.py <linux|windows> <version> <binary> <folder>  the archive for that system, in <folder>

The archive unpacks to one folder. The app looks for `service/bin/box` beside its own binary, so the
folder works wherever it is put. scripts/bundle-macos.sh uses `service` for the Mac bundle.
"""
import os
import shutil
import subprocess
import sys
import tarfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SERVICE = ROOT / "service"
# What stays in the repository: the service's tests and documents. Everything else is carried.
SERVICE_LEFT_OUT = ("tests", "docs", "README.md")
LEFT_OUT_ANYWHERE = ("__pycache__", "*.pyc", ".DS_Store", ".gitignore", "node_modules")
ENGINE_LEFT_OUT = ("test", "vitest.config.ts")
DOCUMENTS = ("LICENSE", "THIRD_PARTY_NOTICES", "README.md")
LINUX_EXTRAS = (("packaging/linux/sakura.desktop", "sakura.desktop"), ("assets/icons/sakura-1024.png", "sakura.png"))
ARCHITECTURE = "x86_64"
INSTALL_DEADLINE_SECONDS = 600


def left_out(folder, names):
    """The names `shutil.copytree` skips in `folder`."""
    skipped = set(shutil.ignore_patterns(*LEFT_OUT_ANYWHERE)(folder, names))
    if Path(folder) == SERVICE:
        skipped.update(SERVICE_LEFT_OUT)
    if Path(folder) == SERVICE / "engine":
        skipped.update(ENGINE_LEFT_OUT)
    return skipped


def install_engine(engine):
    """Install the engine's own dependencies from its lock file, without the ones only its tests need."""
    npm = shutil.which("npm")
    if npm is None:
        raise ValueError("npm is needed to carry the engine's dependencies (install Node.js)")
    subprocess.run([npm, "ci", "--omit=dev", "--ignore-scripts", "--no-audit", "--no-fund"], cwd=engine,
                   check=True, timeout=INSTALL_DEADLINE_SECONDS, stdin=subprocess.DEVNULL)


def carry_service(destination):
    """Copy the service as the app carries it; with the engine, its dependencies too."""
    if destination.exists():
        shutil.rmtree(destination)
    shutil.copytree(SERVICE, destination, ignore=left_out)
    if (destination / "engine" / "package.json").is_file():
        install_engine(destination / "engine")


def carry_documents(destination):
    """Carry the licence, dependency notices and README beside the runtime resources."""
    destination.mkdir(parents=True, exist_ok=True)
    for name in DOCUMENTS:
        shutil.copy2(ROOT / name, destination / name)
    shutil.copytree(ROOT / "LICENSES", destination / "LICENSES", dirs_exist_ok=True)
    environment = dict(os.environ, SAKURA_ENGINE_TREE=str(destination / "service/engine"))
    with (destination / "THIRD_PARTY_LICENSES").open("w") as stream:
        subprocess.run([sys.executable, str(ROOT / "scripts/third_party_notices.py"), "--licenses"],
                       check=True, env=environment, stdout=stream, stdin=subprocess.DEVNULL,
                       timeout=INSTALL_DEADLINE_SECONDS)


def lay_out(system, binary, folder):
    """Copy the binary, the service and the documents into `folder`."""
    folder.mkdir(parents=True)
    shutil.copy2(binary, folder / ("sakura.exe" if system == "windows" else "sakura"))
    carry_service(folder / "service")
    carry_documents(folder)
    if system == "linux":
        for source, name in LINUX_EXTRAS:
            shutil.copy2(ROOT / source, folder / name)


def archive(system, folder):
    """Make the tarball (Linux, keeping the execute permissions) or the zip (Windows) beside `folder`."""
    if system == "linux":
        path = folder.parent / f"{folder.name}.tar.gz"
        with tarfile.open(path, "w:gz") as tar:
            tar.add(folder, arcname=folder.name)
        return path
    path = folder.parent / f"{folder.name}.zip"
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as zipped:
        for item in sorted(folder.rglob("*")):
            if item.is_file():
                zipped.write(item, Path(folder.name) / item.relative_to(folder))
    return path


def package(system, version, binary, output):
    """Build the archive for `system` in `output` and return its path."""
    name = f"sakura-{version}-linux-{ARCHITECTURE}" if system == "linux" else f"Sakura-{version}-windows-{ARCHITECTURE}"
    folder = output / name
    if folder.exists():
        shutil.rmtree(folder)
    lay_out(system, binary, folder)
    return archive(system, folder)


def main(arguments):
    """Run one of the two forms; anything else prints the usage."""
    if len(arguments) == 2 and arguments[0] == "service":
        carry_service(Path(arguments[1]))
        print(arguments[1])
    elif len(arguments) == 2 and arguments[0] == "documents":
        carry_documents(Path(arguments[1]))
        print(arguments[1])
    elif len(arguments) == 4 and arguments[0] in ("linux", "windows"):
        print(package(arguments[0], arguments[1], Path(arguments[2]), Path(arguments[3])))
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        sys.exit(f"package: {error}")
