"""A throwaway engine home and a throwaway `$HOME` keep every record of a run of the engine.

The password database names a stand-in account home (never the person's real one); the engine home, `$HOME` and the trust
root of the test are temporary folders, and each entry point of the engine runs in this process. The records that decide
nothing about trust follow `SAKURA_ENGINE_HOME`; the trust root and the proof secret never follow an environment
variable (the engine's own rule: `ORIGINAL_ROOT_FOR_TRUST`), so a run is isolated by its own `$HOME` and trust root.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

from sakura import cli as sakura_cli
from sakura import engine_home, install
from conftest import ORIGINAL_ROOT_FOR_TRUST
from sakura.cockpit import actions, decided, proofs
from sakura.flow import cli as flow_cli
from sakura.flow import exits, gates, ports, registry, store
from sakura.lanes import events
from sakura.master import cli as master_cli
from sakura.master import spec as master_spec
from test_cockpit_actions import Spawns
from test_cockpit_cli import act, target_of
from test_cockpit_decided import PERSON_TEXT
from test_cockpit_snapshot import create_task
from test_flow_lane_questions import _asked

MACHINE_WIDE_LOCKS = ".cache"  # the heavy-work slots are one lock set for the whole machine, not engine state
HOME_RECORDS = ".local/state/sakura"  # what stays under $HOME on purpose: the proof secret, hook marks, gate state, ports


def _names(folder: Path) -> dict[str, int]:
    """Every file and folder under `folder` with its size, so a change of content shows too."""
    return {name: path.stat().st_size for path in sorted(folder.rglob("*"))
            if not (name := str(path.relative_to(folder))).startswith(MACHINE_WIDE_LOCKS)}


def _trust_the_hooks(repo: Path) -> None:
    (repo / ".claude").mkdir()
    (repo / ".claude" / "settings.json").write_text(json.dumps({"hooks": {}}))
    assert sakura_cli.main(["trust", str(repo)]) == 0


def _answer_a_question(repo: Path, monkeypatch) -> None:
    _ctx, wait = _asked(repo)
    monkeypatch.setenv("SAKURA_ACTOR_TRUSTED", "1")
    monkeypatch.chdir(repo)
    install.install(repo)
    question, digest = json.loads(wait.id)[4].split("@")
    assert flow_cli.main(["reply", "--choice=yes", f"--actor={PERSON_TEXT}", "--", "demo", question, digest]) == exits.OK


def _act_from_the_cockpit(repo: Path, capsys) -> None:
    create_task(repo, "shown", status="running")
    registry.remember(repo)
    capsys.readouterr()
    argv = ["D", target_of(repo, "shown"), "--step", "power", "--actor", "owner@example.com"]
    code, answer = act(capsys, argv, run=Spawns())
    assert code == exits.OK and answer["result"] == "done", answer


def _start_a_lane(repo: Path, monkeypatch) -> None:
    install.install(repo)
    monkeypatch.chdir(repo)
    monkeypatch.setattr(flow_cli, "_advance", lambda *args, **kwargs: exits.OK)
    assert flow_cli.main(["start", "-m", "first thing", "--actor=cli:person"]) == exits.OK


def _write_the_masters_cage(capsys) -> None:
    assert master_cli.main(["spec", "--json"]) == exits.OK
    capsys.readouterr()


def test_EntryPoints_WithAThrowawayEngineHomeAndHome_WriteNothingToTheAccountHomeOrOutsideTheirFolders(
        repo, account_home, monkeypatch, capsys):
    home = Path(os.environ["HOME"])
    before = _names(account_home)

    _trust_the_hooks(repo)
    _answer_a_question(repo, monkeypatch)
    _act_from_the_cockpit(repo, capsys)
    _start_a_lane(repo, monkeypatch)
    _write_the_masters_cage(capsys)

    assert _names(account_home) == before, "records written under the account home"
    outside = [name for name in _names(home) if name not in (".local", ".local/state") and not name.startswith(HOME_RECORDS)]
    assert not outside, f"records under $HOME outside {HOME_RECORDS}: {outside}"
    assert not list(home.rglob("decided.jsonl")) and not list(home.rglob("actions.jsonl"))
    engine, state = engine_home.root(), engine_home.state_dir()
    assert (engine / engine_home.project_id(repo) / engine_home.KIT_DIR_NAME / "trusted-hooks.json").is_file()
    assert (state / "cockpit" / "decided.jsonl").is_file() and (state / "cockpit" / "actions.jsonl").is_file()
    assert (home / HOME_RECORDS / "cockpit" / proofs.SECRET_FILE).is_file()
    assert master_spec.settings_path(engine).is_file()


def test_StateDir_WithAnEngineHome_IsInsideIt():
    assert engine_home.state_dir() == engine_home.root() / "state"


def test_StateDir_WithoutAnEngineHome_StaysUnderHomeAsBefore(monkeypatch):
    monkeypatch.delenv("SAKURA_ENGINE_HOME")
    assert engine_home.state_dir() == Path.home() / ".local" / "state" / "sakura"


def test_Decided_DefaultFolder_IsTheStateFolderOfTheEngineHome():
    assert decided.default_state_dir() == engine_home.state_dir()


def test_ActionLog_DefaultFile_IsInTheStateFolderOfTheEngineHome():
    assert actions._destination(None) == engine_home.state_dir() / "cockpit" / "actions.jsonl"


def test_LaneEvents_DefaultFile_IsInTheStateFolderOfTheEngineHome():
    assert events._resolve(None) == engine_home.state_dir() / "lanes" / "events.jsonl"


def test_EngineHomeVariable_MovesNeitherTheTrustRootNorTheProofSecret(tmp_path, monkeypatch, account_home):
    monkeypatch.setattr(engine_home, "root_for_trust", ORIGINAL_ROOT_FOR_TRUST)
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir(mode=0o700)
    secret_before = proofs.ProofStore()._directory
    monkeypatch.setenv("SAKURA_ENGINE_HOME", str(elsewhere))
    assert engine_home.root_for_trust() == engine_home.account_root() == account_home / engine_home.DEFAULT_RELATIVE
    assert proofs.ProofStore()._directory == secret_before
    assert elsewhere not in proofs.ProofStore()._directory.parents


def test_ATempHome_MovesTheProofSecretButNotTheTrustRoot(tmp_path, monkeypatch, account_home):
    monkeypatch.setattr(engine_home, "root_for_trust", ORIGINAL_ROOT_FOR_TRUST)
    monkeypatch.setenv("HOME", str(tmp_path / "another-home"))
    assert proofs.ProofStore()._directory == tmp_path / "another-home" / ".local/state/sakura/cockpit"
    assert engine_home.root_for_trust() == account_home / engine_home.DEFAULT_RELATIVE


def test_HostRecordsThatDecideTrust_StayUnderHomeWithAnEngineHome(repo):
    task = store.TaskStore(repo, "demo")
    found = [gates.default_mode_file(), ports._registry_path(), task._hook_mark_dir(), task._digest_file()]
    assert all(Path.home() in path.parents for path in found), found
