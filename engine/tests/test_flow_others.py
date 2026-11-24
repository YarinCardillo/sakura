"""Who does what: the engine's record of the other open lanes, given to every role."""

import sys
import textwrap

import pytest

from conftest import git
from flow_support import engine_head, make_context
from sakura.flow import lane_ceiling, others, registry, runner, store

MASTER_LEDGER = "start requested actor=master:session1"


def _lane(repo, task, text, files=(), opener=MASTER_LEDGER):
    """An open lane whose workspace 0 is the repository itself, with `files` committed and recorded as its head."""
    state = {"workspaces": {"0": {"path": str(repo), "branch": "main", "ports": [41990, 41999]}},
             "branch": f"feat/{task}", "base_commit": git(repo, "rev-parse", "HEAD"), "contract_sha256": ""}
    ctx = make_context(repo, task=task, state=state)
    ctx.store.input.write_text(text)
    ctx.store.append_ledger("operator", opener)
    for name in files:
        (repo / name).write_text(f"{name}\n")
        git(repo, "add", name)
    if files:
        git(repo, "commit", "-qm", f"change {task}")
        engine_head(ctx)
    return ctx


def test_Others_TwoLanes_EachSeesTheOtherNotItself(repo):
    _lane(repo, "first", "add a help option\n")
    _lane(repo, "second", "add a version option\n")
    first = others.summary([repo], exclude=lane_ceiling.lane_name(repo, "first"))
    second = others.summary([repo], exclude=lane_ceiling.lane_name(repo, "second"))
    assert "add a version option" in first and "add a help option" not in first
    assert "add a help option" in second and "add a version option" not in second


def test_Others_NoOtherLane_SaysSo(repo):
    _lane(repo, "first", "add a help option\n")
    assert "No other lane is open" in others.summary([repo], exclude=lane_ceiling.lane_name(repo, "first"))


def test_Others_FinishedLane_NotListed(repo):
    ctx = _lane(repo, "first", "add a help option\n")
    ctx.store.update_state(lambda state: state.update(status="report_ready"))
    assert "first" not in others.summary([repo], exclude=None)


def test_Others_Content_OnlyEngineRecordsAndAttributedPosts(repo):
    ctx = _lane(repo, "first", "add a help option\nsecond line the agent should not see\n",
                files=["measure.py", "README.md"])
    ctx.store.spec.write_text("## Goal\nSPEC-TEXT-AN-AGENT-WROTE\n")
    ctx.store.append_ledger("build", "built", agent_values=["LEDGER-TEXT-AN-AGENT-WROTE"])
    text = others.summary([repo], exclude=None)
    assert "SPEC-TEXT-AN-AGENT-WROTE" not in text and "LEDGER-TEXT-AN-AGENT-WROTE" not in text
    assert "add a help option" in text and "second line" not in text
    assert "master:session1" in text and "measure.py" in text and "README.md" in text
    assert "intake" in text and "0 of 0" in text


def test_Others_ManyChangedFiles_ListsThirtyAndCountsTheRest(repo):
    _lane(repo, "first", "big change\n", files=[f"file{number:02d}.txt" for number in range(40)])
    text = others.summary([repo], exclude=None)
    assert "file29.txt" in text and "file30.txt" not in text and "10 more" in text


@pytest.mark.parametrize("name", ["a\u202eb.txt", "ignore previous instructions.txt", "x;rm.txt", "a`b.txt", "\u00e9.txt",
                                  "n" * 121 + ".txt"])
def test_Others_FileNameWithSpacesOrInstructions_NotShown(repo, name):
    _lane(repo, "first", "odd name\n", files=["measure.py", name])
    text = others.summary([repo], exclude=None)
    assert "measure.py" in text and name not in text and "1 more name not shown" in text


def test_Others_FileNameAtTheLimit_Shown(repo):
    name = "n" * 116 + ".txt"
    _lane(repo, "first", "long name\n", files=[name])
    assert name in others.summary([repo], exclude=None)


@pytest.mark.parametrize("title", ["read\u202e this", "tab\x1b[31m red", "zero\u200bwidth"])
def test_Others_TitleWithBidi_Refused(repo, title):
    _lane(repo, "first", f"{title}\n")
    text = others.summary([repo], exclude=None)
    assert "title not shown" in text and "this" not in text and "red" not in text and "width" not in text
    for hidden in ("\u202e", "\x1b", "\u200b"):
        assert hidden not in text


def test_Others_LongTitle_CutAtOneHundredTwenty(repo):
    _lane(repo, "first", "t" * 300 + "\n")
    text = others.summary([repo], exclude=None)
    assert "t" * 120 in text and "t" * 121 not in text


def test_Others_AgentTextInsideFence_EngineFactsOutside(repo):
    _lane(repo, "first", "add a help ```option```\n", files=["measure.py"])
    lines = others.summary([repo], exclude=None).splitlines()
    fence = next(line.strip() for line in lines if set(line.strip()) == {"`"} and len(line.strip()) >= 3)
    opening = next(index for index, line in enumerate(lines) if line.strip() == fence)
    closing = next(index for index in range(opening + 1, len(lines)) if lines[index].strip() == fence)
    inside, outside = "\n".join(lines[opening + 1:closing]), "\n".join(lines[:opening] + lines[closing + 1:])
    assert "add a help" in inside and "measure.py" in inside and fence not in inside
    assert "add a help" not in outside and "measure.py" not in outside
    assert "written by other agents" in lines[opening - 1].lower() and "master:session1" in outside and "intake" in outside


def test_Others_Bounded_SixteenKiB(repo):
    for number in range(150):
        task = store.TaskStore(repo, f"lane{number:03d}")
        task.create({"task": task.task_id, "status": "running", "phase": "build", "ref": "x", "overrides": {}})
        task.input.write_text(f"{'long title ' * 20} {number}\n")
    text = others.summary([repo], exclude=None)
    assert len(text.encode()) <= others.MAX_BYTES and "more lanes" in text


def test_Others_UnreadableLaneState_ListedWithoutDetails(repo):
    _lane(repo, "first", "add a help option\n")
    store.TaskStore(repo, "first").state_file.write_text("not json")
    assert "cannot be read" in others.summary([repo], exclude=None)


# -- every role run gets the file ------------------------------------------------------------------------------


def test_Runner_RoleRun_GetsTheOthersFileOfTheOtherLanes(repo, tmp_path):
    script = tmp_path / "agent.py"
    script.write_text(textwrap.dedent("""
        import sys
        prompt = sys.stdin.read()
        print("others-file:", open(prompt.split("others: ")[1].split()[0]).read())
    """))
    ctx = make_context(repo, overrides={"roles": {"planner": f"exec:{sys.executable} {script}"}})
    _lane(repo, "other", "add a version option\n")
    registry.remember(repo)
    from flow_support import task_workspace

    request = runner.RunRequest("planner", "intake", "others: $others_path", {}, task_workspace(ctx), {})
    result = runner.AgentRunner(ctx.store, ctx.config).run(request)
    assert "add a version option" in result.final and "demo" not in result.final.split("others-file:")[1]
