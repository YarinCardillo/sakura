import json
import subprocess
import sys
from pathlib import Path

import pytest

from sakura.flow import brakes, cli, registry

LIB = Path(registry.__file__).resolve().parents[2]
WRITER = (
    "import sys; sys.path.insert(0, sys.argv[1]);"
    "from pathlib import Path; from sakura.flow import registry;"
    "registry.remember(Path(sys.argv[2]), Path(sys.argv[3]))"
)


def make_dirs(base: Path, count: int) -> list[Path]:
    paths = [base / f"repo-{number}" for number in range(count)]
    for path in paths:
        path.mkdir()
    return paths


def test_remember_concurrent_writers_lose_nothing(tmp_path):
    registry_file = tmp_path / "state" / "repos.json"
    repos = make_dirs(tmp_path, 12)
    writers = [subprocess.Popen([sys.executable, "-c", WRITER, str(LIB), str(repo), str(registry_file)])
               for repo in repos]
    assert [writer.wait(timeout=60) for writer in writers] == [0] * len(repos)
    assert sorted(registry.repos(registry_file)) == sorted(path.resolve() for path in repos)


def test_remember_deduplicates_by_resolved_path(tmp_path):
    registry_file = tmp_path / "repos.json"
    (real,) = make_dirs(tmp_path, 1)
    link = tmp_path / "link"
    link.symlink_to(real)
    registry.remember(real, registry_file)
    registry.remember(link, registry_file)
    registry.remember(real / ".." / real.name, registry_file)
    assert registry.repos(registry_file) == [real.resolve()]


def test_repos_drops_missing_ones(tmp_path):
    registry_file = tmp_path / "repos.json"
    kept, gone = make_dirs(tmp_path, 2)
    registry.remember(kept, registry_file)
    registry.remember(gone, registry_file)
    gone.rmdir()
    assert registry.repos(registry_file) == [kept.resolve()]
    assert registry.repos(tmp_path / "never-written.json") == []


def test_default_path_lives_in_the_engine_home(tmp_path):
    (repo,) = make_dirs(tmp_path, 1)
    registry.remember(repo)
    assert registry.default_path().parent == tmp_path / "engine-home"
    assert registry.default_path().is_file()
    assert registry.repos() == [repo.resolve()]


def test_corrupt_registry_is_reported_not_overwritten(tmp_path):
    registry_file = tmp_path / "repos.json"
    registry_file.write_text("{not json")
    (repo,) = make_dirs(tmp_path, 1)
    with pytest.raises(registry.RegistryError, match="repos.json"):
        registry.remember(repo, registry_file)
    with pytest.raises(registry.RegistryError):
        registry.repos(registry_file)
    assert registry_file.read_text() == "{not json"


def test_registry_file_only_lists_strings(tmp_path):
    registry_file = tmp_path / "repos.json"
    registry_file.write_text(json.dumps({"repos": [1, None, str(tmp_path)]}))
    assert registry.repos(registry_file) == [tmp_path.resolve()]


COMMAND_ARGV = {
    "start": ["start", "ref"], "resume": ["resume", "t"], "recontract": ["recontract", "t"], "stop": ["stop", "t"],
    "steer": ["steer", "t", "x"], "dial": ["dial", "t", "lean"], "role": ["role", "t", "builder", "claude:sonnet"],
    "approve": ["approve", "t", "spec"], "status": ["status"], "sessions": ["sessions", "t"],
    "watch": ["watch", "t", "r"], "events": ["events", "t"], "attach": ["attach", "t"], "open": ["open", "t", "r"],
    "report": ["report", "t"], "cleanup": ["cleanup", "t"], "run-row": ["run-row", "R1"], "mode": ["mode"],
    "migrate-state": ["migrate-state"], "mark": ["mark", "t", "R1", "ok"], "restore-head": ["restore-head", "t"],
    "reply": ["reply", "t", "q1", "d1", "--text", "yes"], "feed": ["feed", "t", "--json"],
    "pace": ["pace", "t", "fast"], "preset": ["preset", "t", "lean"], "step": ["step", "t", "checks", "on"], "changes": ["changes", "t", "--json"],
    "agent-worker": ["agent-worker", "t", "build-1"],
}
# Commands that do not register the repository of the working directory: `register` names its own, and `wake`
# runs from a background job whose directory is no repository.
NOT_REGISTERING = {"register", "wake"}


def test_command_table_covers_every_flow_command():
    assert set(COMMAND_ARGV) == set(cli.COMMANDS) - NOT_REGISTERING


@pytest.mark.parametrize("command", sorted(COMMAND_ARGV))
def test_every_flow_command_registers_repo(command, repo, tmp_path, monkeypatch):
    monkeypatch.chdir(repo)
    monkeypatch.setitem(cli.COMMANDS, command, lambda args: 0)
    assert cli.main(COMMAND_ARGV[command]) == 0
    assert registry.repos() == [repo.resolve()]


def test_wake_does_not_register_the_directory_it_runs_in(repo, monkeypatch):
    monkeypatch.chdir(repo)
    monkeypatch.setitem(cli.COMMANDS, "wake", lambda args: 0)
    assert cli.main(["wake"]) == 0
    assert registry.repos() == []


def test_register_command_remembers_the_given_path(repo, tmp_path, monkeypatch, capsys):
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    monkeypatch.chdir(elsewhere)
    assert cli.main(["register", str(repo)]) == 0
    assert registry.repos() == [repo.resolve()]
    assert str(repo.resolve()) in capsys.readouterr().out


def test_register_refuses_a_path_that_is_not_a_repository(tmp_path, monkeypatch, capsys):
    plain = tmp_path / "plain"
    plain.mkdir()
    monkeypatch.chdir(plain)
    assert cli.main(["register", str(plain)]) != 0
    assert registry.repos() == []


def test_engine_role_session_does_not_register(repo, monkeypatch):
    monkeypatch.chdir(repo)
    monkeypatch.setenv(brakes.ROLE_ENV, "builder")
    monkeypatch.setitem(cli.COMMANDS, "status", lambda args: 0)
    assert cli.main(["status"]) == 0
    assert registry.repos() == []


def test_unwritable_registry_warns_and_the_command_still_runs(repo, tmp_path, monkeypatch, capsys):
    (tmp_path / "engine-home" / "repos.json").mkdir()
    monkeypatch.chdir(repo)
    monkeypatch.setitem(cli.COMMANDS, "status", lambda args: 0)
    assert cli.main(["status"]) == 0
    assert "cannot remember repository" in capsys.readouterr().err


def test_unreadable_engine_home_warns_and_the_command_still_runs(repo, monkeypatch, capsys):
    """Inside an agent cage the engine home cannot even be looked at: remembering warns, the command decides."""
    def denied():
        raise PermissionError(1, "Operation not permitted")

    monkeypatch.setattr(registry.engine_home, "root", denied)
    monkeypatch.chdir(repo)
    monkeypatch.setitem(cli.COMMANDS, "status", lambda args: 0)
    assert cli.main(["status"]) == 0
    assert "cannot remember repository" in capsys.readouterr().err
