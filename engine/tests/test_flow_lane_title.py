"""A lane keeps the title it was given: the first line of the task text, shown where the slug used to be."""

import contextlib
import io
import json
import subprocess

import pytest

from sakura import engine_home, install
from sakura.cockpit import api, cli, snapshot
from sakura.cockpit.project_actions import ProjectTarget
from sakura.flow import cli as flow_cli
from sakura.flow import exits, registry, store, tasks

from test_cockpit_projects import NoTmux
from test_cockpit_snapshot import NIGHT, create_task
from test_master_server import World

TITLED = "Add a farewell\n\nPrint goodbye when the program exits."


def test_SplitTitle_TitleBlankLineDescription_SplitsThem():
    assert tasks.split_title(TITLED) == ("Add a farewell", "Print goodbye when the program exits.")


def test_SplitTitle_OneLine_IsTheTitleAndThereIsNoDescription():
    assert tasks.split_title("  Add   a farewell \n") == ("Add a farewell", "")


def test_SplitTitle_ALongFirstLine_IsCutAtTheLimit():
    title, _ = tasks.split_title("x" * 500)
    assert len(title) == tasks.MAX_TITLE_CHARACTERS


@pytest.fixture
def started(repo, monkeypatch):
    """A registered project whose `flow start` runs in this process: the lane is made, nothing is driven."""
    install.install(repo)
    monkeypatch.chdir(repo)
    monkeypatch.setattr(flow_cli, "_detach_resume", lambda *args, **kwargs: exits.OK)
    registry.remember(repo)
    return repo


def _in_process(argv, **kwargs):
    words = [word for word in argv[2:] if word != "--detach"]
    output = io.StringIO()
    with contextlib.redirect_stdout(output):
        code = flow_cli.main(words)
    return subprocess.CompletedProcess(argv, code, output.getvalue(), "")


def _start_through_the_act(repo, capsys, text):
    (project,) = api.snapshot_document(snapshot.collect(registry.repos(), NIGHT))["projects"]
    code = cli.main(["act", "L", project["id"], "--text", text, "--actor", "me", "--json"], client=NoTmux(), run=_in_process)
    answer = json.loads(capsys.readouterr().out)
    assert code == exits.OK and answer["result"] == "done", answer
    document = api.snapshot_document(snapshot.collect(registry.repos(), NIGHT))
    (item,) = [item for item in document["items"] if item["kind"] == "flow"]
    return item, document


def test_ActStartLane_TitleAndDescription_TheSnapshotKeepsTheTitleAndTheRestIsTheGoal(started, capsys):
    item, _ = _start_through_the_act(started, capsys, TITLED)
    assert item["title"] == "Add a farewell"
    assert item["goal"] == "Print goodbye when the program exits."
    assert item["key"][2].startswith("add-a-farewell")  # the slug is still the id


def test_ActStartLane_AOneLineText_IsTheTitleAndTheGoal(started, capsys):
    item, _ = _start_through_the_act(started, capsys, "Add a farewell")
    assert (item["title"], item["goal"]) == ("Add a farewell", "Add a farewell")


def test_Snapshot_AnAskOfATitledLane_NamesTheLaneByItsTitle(repo):
    create_task(repo, "slug-name", status="awaiting_approval", title="Add a farewell",
                reason="awaiting approval: sakura flow approve slug-name spec")
    registry.remember(repo)
    document = api.snapshot_document(snapshot.collect(registry.repos(), NIGHT))
    (ask,) = document["waits"]
    assert (ask["task"], ask["lane_title"]) == ("slug-name", "Add a farewell")


def test_Snapshot_ALaneWithNoTitle_HasAnEmptyTitleEverywhere(repo):
    create_task(repo, "slug-name", status="awaiting_approval", reason="awaiting approval: sakura flow approve slug-name spec")
    registry.remember(repo)
    document = api.snapshot_document(snapshot.collect(registry.repos(), NIGHT))
    assert document["items"][0]["title"] == "" and document["waits"][0]["lane_title"] == ""


def test_MasterServer_LanesAndWaits_CarryTheTitle(repo, tmp_path):
    world = World(repo, tmp_path)
    create_task(repo, "slug-name", status="awaiting_approval", title="Add a farewell",
                reason="awaiting approval: sakura flow approve slug-name spec")
    lanes = json.loads(world.tools.lanes({})[0])["lanes"]
    waits = json.loads(world.tools.waits({})[0])["waits"]
    assert (lanes[0]["name"], lanes[0]["title"]) == ("slug-name", "Add a farewell")
    assert (waits[0]["name"], waits[0]["lane_title"]) == ("slug-name", "Add a farewell")


def test_Snapshot_AnOutsideLane_KeepsItsOwnTitle(tmp_path):
    from datetime import datetime, timezone
    from sakura.outside import records, store as outside_store

    now = datetime(2026, 11, 16, 10, 0, tzinfo=timezone.utc)
    outside_store.open_lane(outside_store.path(), records.Agent("claude", tmp_path), "ext", title="Outside work", now=now)
    document = api.snapshot_document(snapshot.collect([], NIGHT))
    assert [item["title"] for item in document["items"] if item["kind"] == "outside"] == ["Outside work"]
