from __future__ import annotations

import contextlib
import os
import pwd
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

pytest_plugins = ("digest_supervise_support", "process_support", "stack_support")

# Keep the run, and everything it spawns, off the person's tmux server. A tmux without -L
# uses $TMUX, else $TMUX_TMPDIR/tmux-<uid>/default; a short path, as macOS caps socket paths.
TMUX_ROOT = tempfile.mkdtemp(prefix="akt-tmux-", dir="/tmp")
for _name in ("TMUX", "TMUX_PANE"):
    os.environ.pop(_name, None)
os.environ["TMUX_TMPDIR"] = os.environ["SAKURA_TEST_TMUX_ROOT"] = TMUX_ROOT


GIT = shutil.which("git", path="/opt/homebrew/bin:/usr/local/bin:/usr/bin")


def git(repo: Path, *args: str) -> str:
    if GIT is None:
        pytest.fail("Git is required for repository fixtures")
    env = {name: value for name, value in os.environ.items() if not name.startswith("GIT_")}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull)
    done = subprocess.run([GIT, "-C", str(repo), *args], env=env, capture_output=True, text=True, check=True)
    return done.stdout.strip()


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    path = tmp_path / "repo"
    git(path.parent, "init", "-q", "-b", "main", str(path))
    git(path, "config", "user.name", "Test User")
    git(path, "config", "user.email", "test@example.invalid")
    (path / "README").write_text("initial\n")
    git(path, "add", ".")
    git(path, "commit", "-qm", "initial")
    return path


@pytest.fixture(autouse=True)
def isolated_home(tmp_path, monkeypatch):
    """Keep per-user state (flow digests) and the engine home inside the test's temp directory."""
    home = tmp_path / "isolated-home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    engine = tmp_path / "engine-home"
    engine.mkdir(mode=0o700)
    monkeypatch.setenv("SAKURA_ENGINE_HOME", str(engine))


@pytest.fixture(autouse=True)
def isolated_trust_root(tmp_path, monkeypatch):
    """The trust root ignores the environment, so in-process tests patch it to the same temp engine home."""
    from sakura import engine_home

    monkeypatch.setattr(engine_home, "root_for_trust", lambda: tmp_path / "engine-home")


def trust_root_child_environment() -> dict[str, str]:
    """The variables that make a `bin/sakura` child of a test use the test's own trust root (`tests/fixtures/trust_root_python`):
    the in-process patch of `engine_home.root_for_trust` does not reach another process, and the real trust root ignores the
    environment on purpose."""
    from sakura import engine_home

    fixture = Path(__file__).resolve().parent / "fixtures" / "trust_root_python"
    return {"SAKURA_PYTHON": str(fixture), "SAKURA_TEST_TRUST_ROOT": str(engine_home.root_for_trust()),
            "SAKURA_TEST_LIB": str(Path(__file__).resolve().parent.parent / "lib")}


def _real_account_entries(root: Path) -> set[str]:
    """List entries up to two levels deep without following symlinks."""
    found = set()
    try:
        for entry in os.scandir(root):
            found.add(entry.name)
            if entry.is_dir(follow_symlinks=False):
                with contextlib.suppress(OSError):
                    found.update(f"{entry.name}/{inner.name}" for inner in os.scandir(entry.path))
    except OSError:
        pass
    return found


@contextlib.contextmanager
def watching_the_real_account_root(root: Path):
    """Fail if a path that bypasses the trust-root fixture adds an entry here."""
    before = _real_account_entries(root)
    yield
    added = sorted(_real_account_entries(root) - before)
    if added:
        pytest.fail(f"this test wrote under the real account root: {', '.join(added)}", pytrace=False)


@pytest.fixture(autouse=True)
def real_account_root_untouched(tmp_path, monkeypatch):
    from sakura import engine_home

    entry = pwd.getpwuid(os.getuid())
    home = tmp_path / "default-account"
    monkeypatch.setattr(pwd, "getpwuid", lambda _uid: pwd.struct_passwd((*entry[:5], str(home), entry[6])))
    with watching_the_real_account_root(home / engine_home.DEFAULT_RELATIVE):
        yield


@pytest.fixture(autouse=True)
def programs_in_the_tests_tmp_folder(monkeypatch):
    """The trust check refuses programs under the system tmp folders (agents can write there); the tests keep their fake tools
    in their own tmp folder, so it is not applied to them here. The tests of the rule turn it back on."""
    from sakura import programs

    monkeypatch.setattr(programs, "FORBID_SYSTEM_TMP", False)


@pytest.fixture(autouse=True)
def no_real_model_read(monkeypatch):
    """No test starts the background read of an account's models: it runs the real `claude` or `codex` on a login (and, on a
    throwaway home, makes macOS ask for a keychain). A test of the read replaces it itself."""
    from sakura.flow import model_catalog

    monkeypatch.setattr(model_catalog, "start_refresh", lambda: None)


@pytest.fixture(autouse=True)
def no_real_leeway(monkeypatch):
    """No test finds, probes or reads the installed Leeway: the helper is the fake of the test that sets `SAKURA_LEEWAY`.

    The environment variable keeps the child processes of a test off the installed app too, and a probe of the real path
    in this process fails the test that made it."""
    from sakura.usage import leeway

    real_paths, probe = tuple(leeway.INSTALLED_HELPERS), leeway._version_of

    def guarded_probe(candidate):
        assert candidate not in real_paths, f"a test probed the installed Leeway at {candidate}"
        return probe(candidate)

    monkeypatch.setattr(leeway, "INSTALLED_HELPERS", ())
    monkeypatch.setattr(leeway, "_version_of", guarded_probe)
    monkeypatch.setenv(leeway.INSTALLED_ENV, "")
    monkeypatch.delenv(leeway.ENV_VARIABLE, raising=False)
    leeway.forget()


@pytest.fixture(autouse=True)
def headless_surface(monkeypatch):
    """Keep engine tests off every tmux server: contexts built from config get the null surface.

    Surface tests import `build_surface` at collection time and inject private clients themselves.
    """
    from sakura.flow import surface

    monkeypatch.setattr(surface, "build_surface", lambda config, client=None: surface.NullSurface())


@pytest.fixture(scope="session", autouse=True)
def session_tmux_root():
    yield TMUX_ROOT
    shutil.rmtree(TMUX_ROOT, ignore_errors=True)


@pytest.fixture(autouse=True)
def private_tmux_root(session_tmux_root, monkeypatch):
    """Give each test its own tmux namespace, including its default socket."""
    from tmux_support import TMUX_PROGRAM

    private = Path(tempfile.mkdtemp(prefix="t-", dir=session_tmux_root))
    monkeypatch.setenv("TMUX_TMPDIR", str(private))
    yield private
    socket_dir = private / f"tmux-{os.getuid()}"
    for entry in socket_dir.glob("*") if TMUX_PROGRAM is not None else ():
        if entry.is_socket():
            subprocess.run([TMUX_PROGRAM, "-S", str(entry), "kill-server"],
                           env={**os.environ, "TMUX_TMPDIR": str(private)}, capture_output=True, check=False)
    shutil.rmtree(private, ignore_errors=True)


@pytest.fixture(autouse=True)
def no_trusted_git_repositories(monkeypatch):
    """Each test starts with no repository the engine's Git helper has been told to trust."""
    from sakura.flow import gitutil

    monkeypatch.setattr(gitutil, "_trusted_repositories", [])
    monkeypatch.setattr(gitutil, "_trusted_stores", [])


@pytest.fixture(autouse=True)
def small_ephemeral_range(request, monkeypatch):
    """Every caged command's profile carries one rule per ephemeral port, about five seconds to compile: tests that do
    not open ephemeral sockets use a two-port range (in the environment, so the engine's subprocesses share it). A
    test that does asks for `real_ephemeral_range`."""
    if "real_ephemeral_range" in request.fixturenames:
        monkeypatch.delenv("SAKURA_EPHEMERAL_PORT_RANGE", raising=False)
    else:
        monkeypatch.setenv("SAKURA_EPHEMERAL_PORT_RANGE", "49152-49153")


@pytest.fixture
def real_ephemeral_range():
    """Marker fixture: keep the system's real ephemeral port range in the caged profiles."""


@pytest.fixture(autouse=True)
def fresh_program_pins(monkeypatch):
    """Each test resolves the programs the engine starts (gh, tmux, claude...) from its own PATH."""
    from sakura import programs

    monkeypatch.setattr(programs, "_resolved", {})


ORIGINAL_ROOT_FOR_TRUST = None  # set below: the engine's own, before `isolated_trust_root` moves it for a test


def _remember_original_root_for_trust() -> None:
    global ORIGINAL_ROOT_FOR_TRUST
    from sakura import engine_home

    ORIGINAL_ROOT_FOR_TRUST = engine_home.root_for_trust


_remember_original_root_for_trust()


@pytest.fixture
def account_home(tmp_path, monkeypatch):
    """A stand-in for the real account home: the password database names it and it already holds an engine folder with
    the cage probe, as on any host that has run the engine. The trust root stays the test's own (`isolated_trust_root`);
    a test that needs the engine's real rule restores `ORIGINAL_ROOT_FOR_TRUST`. Whatever the engine writes under this
    home from a path that did not follow the test's folders shows up there, never in the person's real home."""
    from sakura import engine_home

    home = tmp_path / "account"
    probe = home / engine_home.DEFAULT_RELATIVE / engine_home.PROBE_NAME
    probe.parent.mkdir(parents=True, mode=0o700)
    probe.touch()
    entry = ("tester", "*", os.getuid(), 20, "Tester", str(home), "/bin/sh")
    monkeypatch.setattr(pwd, "getpwuid", lambda _uid: pwd.struct_passwd(entry))
    return home
