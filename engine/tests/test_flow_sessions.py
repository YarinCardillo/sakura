"""`sakura flow sessions --json`: the per-run facts the lane page lists (account, doing, listening, resumes)."""

import json
import os
import re
import shutil
from datetime import datetime, timezone
from pathlib import Path

import pytest

from flow_run_support import (INIT, SESSION, add_run, assistant, make_task, text, tool_result, tool_use)
from sakura.flow import cli, exits, inbox, run_records, run_times

MOMENT = datetime(2026, 11, 19, 9, 30, 0, tzinfo=timezone.utc)
MOMENT_TEXT = "2026-11-19T09:30:00+00:00"


@pytest.fixture
def task(repo, tmp_path, monkeypatch):
    """A task store with one recorded workspace outside the repository, and the working directory set to the repository."""
    monkeypatch.chdir(repo)
    return make_task(repo, tmp_path)


def rows(capsys):
    capsys.readouterr()
    assert cli.main(["sessions", "demo", "--json"]) == exits.OK
    return {row["name"]: row for row in json.loads(capsys.readouterr().out)}


def put_transcript(task, cwd, session=SESSION):
    folder = Path.home() / ".claude" / "projects" / re.sub(r"[^A-Za-z0-9]", "-", str(cwd))
    folder.mkdir(parents=True, exist_ok=True)
    (folder / f"{session}.jsonl").write_text("{}\n")


# -- account and resumes

def test_Sessions_AccountAndResumes_ComeFromTheRunRecord(task, capsys):
    add_run(task, "build-1", INIT, account="work")
    add_run(task, "build-1.2", INIT, account="work", resumes="build-1")
    add_run(task, "review-1", INIT, role="code_reviewer")

    found = rows(capsys)

    assert (found["build-1"]["account"], found["build-1"]["resumes"]) == ("work", None)
    assert (found["build-1.2"]["account"], found["build-1.2"]["resumes"]) == ("work", "build-1")
    assert (found["review-1"]["account"], found["review-1"]["resumes"]) == (None, None)


def test_Sessions_RecordWrittenBeforeTheseFields_ShowsThemAsNull(task, capsys):
    add_run(task, "build-1", INIT)
    task.update_state(lambda state: [state["runs"]["build-1"].pop(key) for key in ("account", "resumes")])

    found = rows(capsys)

    assert (found["build-1"]["account"], found["build-1"]["resumes"]) == (None, None)


# -- doing

def test_Sessions_Doing_ToolStartedAndNotCompleted_NamesTheToolItsFirstLineAndItsStart(task, capsys):
    log = assistant(tool_use("t1", command="pytest -q\nsecond line"))
    path = add_run(task, "build-1", log, state="running")
    run_times.record(path, len(log.encode()), MOMENT)

    doing = rows(capsys)["build-1"]["doing"]

    assert doing == {"kind": "tool", "tool": "Bash", "detail": "pytest -q", "since": MOMENT_TEXT}


def test_Sessions_Doing_ToolWithAPath_UsesThePathAsItsDetail(task, capsys):
    path = add_run(task, "build-1", assistant(tool_use("t1", "Read", file_path=str(task.workspace / "src/a.py"))),
                   state="running")

    assert rows(capsys)["build-1"]["doing"]["detail"] == "src/a.py"
    assert path.exists()


def test_Sessions_Doing_ToolCompleted_IsTheLastTextBlockFirstLine(task, capsys):
    log = (assistant(text("I will run the tests."), tool_use("t1", command="ls"))
           + tool_result("t1") + assistant(text("All green.\nNothing else to do.")))
    add_run(task, "build-1", log, state="running")

    assert rows(capsys)["build-1"]["doing"] == {"kind": "text", "text": "All green."}


def test_Sessions_Doing_EmptyLog_IsNull(task, capsys):
    add_run(task, "build-1", "$ claude -p\n", state="running")

    assert rows(capsys)["build-1"]["doing"] is None


def test_Sessions_Doing_CodexToolStartedAndNotCompleted_IsATool(task, capsys):
    log = json.dumps({"type": "item.started", "item": {"id": "i1", "type": "command_execution",
                                                        "command": "cargo test"}}) + "\n"
    add_run(task, "review-1", log, provider="codex", role="code_reviewer", state="running")

    doing = rows(capsys)["review-1"]["doing"]

    assert (doing["kind"], doing["tool"], doing["detail"]) == ("tool", "command_execution", "cargo test")


def test_Sessions_Doing_ToolOfARunThatEnded_IsNotShownAsRunning(task, capsys):
    add_run(task, "build-1", INIT + assistant(text("Starting."), tool_use("t1", command="sleep 9")),
            state="aborted")

    assert rows(capsys)["build-1"]["doing"] == {"kind": "text", "text": "Starting."}


def test_Sessions_Doing_OnlyTheTailOfAHugeLogIsRead(task, capsys, monkeypatch):
    log = assistant(text("early")) + "x" * (3 << 20) + "\n" + assistant(
        text("late"))
    add_run(task, "build-1", log, state="running")
    sizes = []
    real = os.pread
    monkeypatch.setattr(os, "pread", lambda fd, size, offset: (sizes.append(size), real(fd, size, offset))[1])

    assert rows(capsys)["build-1"]["doing"] == {"kind": "text", "text": "late"}
    assert sizes and sum(sizes) <= 1 << 20, "a few hundred kilobytes of the end, not the 3 MB log"


# -- listening

def test_Sessions_Listening_RunningClaudeRunWithALiveChannel_IsLive(task, capsys):
    add_run(task, "build-1", INIT, state="running")
    inbox.mark_live(task, "build-1", "builder")

    found = rows(capsys)["build-1"]

    assert (found["listening"], found["not_listening"]) == ("live", None)


def test_Sessions_Listening_RunningCodexRun_IsQueue(task, capsys):
    add_run(task, "review-1", "", provider="codex", role="code_reviewer", state="running")

    assert rows(capsys)["review-1"]["listening"] == "queue"


def test_Sessions_Listening_RunningClaudeRunWhoseChannelIsClosed_IsQueue(task, capsys):
    add_run(task, "build-1", INIT, state="running")

    assert rows(capsys)["build-1"]["listening"] == "queue"


@pytest.mark.parametrize("state", ["finished", "aborted"])
def test_Sessions_Listening_EndedRunWithItsSessionAndFolder_IsResume(task, capsys, state):
    add_run(task, "build-1", INIT, state=state)
    put_transcript(task, task.workspace)

    found = rows(capsys)["build-1"]

    assert (found["listening"], found["not_listening"]) == ("resume", None)


def test_Sessions_Listening_OrphanedRunWithItsSessionAndFolder_IsResume(task, capsys):
    add_run(task, "build-1", INIT, state="running")
    task.update_state(lambda state: state["runs"]["build-1"].update(identity={"pid": 2147480000, "command": "gone"}))
    run_records.reconcile(task, lambda entry: False)
    put_transcript(task, task.workspace)

    assert rows(capsys)["build-1"]["listening"] == "resume"


def test_Sessions_RunRecordedAsRunningWhoseProcessIsGone_IsShownOrphanedAndCanBeContinued(task, capsys):
    add_run(task, "build-1", INIT + assistant(tool_use("t1", command="sleep 99")), state="running")
    task.update_state(lambda state: state["runs"]["build-1"].update(identity={"pid": 2147480000, "command": "gone"}))
    put_transcript(task, task.workspace)

    found = rows(capsys)["build-1"]

    assert found["state"] == "orphaned" and found["session_id"] == SESSION
    assert found["listening"] == "resume"
    assert found["doing"] is None or found["doing"]["kind"] == "text", "no tool is running in a run that is gone"
    assert task.read_state()["runs"]["build-1"]["state"] == "running", "reading changes nothing"


def test_Sessions_Listening_FolderGone_IsNoWithWordsAndNoPath(task, capsys):
    add_run(task, "build-1", INIT)
    put_transcript(task, task.workspace)
    shutil.rmtree(task.workspace)

    found = rows(capsys)["build-1"]

    assert found["listening"] == "no"
    assert "folder is gone" in found["not_listening"] and str(task.workspace) not in found["not_listening"]


def test_Sessions_Listening_NoSessionRecorded_IsNoWithWords(task, capsys):
    add_run(task, "build-1", "no events at all\n")

    found = rows(capsys)["build-1"]

    assert found["listening"] == "no" and "conversation" in found["not_listening"]


def test_Sessions_Listening_TranscriptMissing_IsNoWithWords(task, capsys):
    add_run(task, "build-1", INIT)

    found = rows(capsys)["build-1"]

    assert found["listening"] == "no" and "no longer on this Mac" in found["not_listening"]


def test_Sessions_Listening_ExecRun_IsNoWithWords(task, capsys):
    add_run(task, "plan-1", "", provider="exec", role="planner")

    found = rows(capsys)["plan-1"]

    assert found["listening"] == "no" and "not an agent" in found["not_listening"]


def test_Sessions_ExistingFieldsAreUnchanged(task, capsys):
    add_run(task, "build-1", INIT)

    found = rows(capsys)["build-1"]

    for key in ("name", "role", "phase", "provider", "model", "effort", "host", "state", "started_at", "ended_at",
                "exit_code", "session_id", "cwd", "resume", "not_resumable"):
        assert key in found
    assert "env" not in found and "identity" not in found


@pytest.mark.parametrize("status,said", [("running", False), ("blocked", False), ("awaiting_approval", False),
                                         ("report_ready", True), ("cleaned", True)])
def test_Sessions_Listening_FolderGoneWords_AreTrueForTheLanesState(task, capsys, status, said):
    add_run(task, "build-1", INIT)
    put_transcript(task, task.workspace)
    shutil.rmtree(task.workspace)
    task.update_state(lambda state: state.update(status=status))

    words = rows(capsys)["build-1"]["not_listening"]

    assert ("delivered" in words) is said, words
    assert words.startswith("Its folder is gone") and str(task.workspace) not in words


def test_Sessions_EachRunSaysWhichStepItServes_AndOldRecordsAreGivenTheirRolesStep(task, capsys):
    add_run(task, "build-1", INIT)
    add_run(task, "review-1", INIT, role="code_reviewer")
    add_run(task, "review-2", INIT, role="code_reviewer")
    task.update_state(lambda state: state["runs"]["review-2"].update(step="plan_review"))
    task.update_state(lambda state: state["runs"]["review-1"].pop("step", None))

    found = rows(capsys)

    assert (found["build-1"]["step"], found["review-1"]["step"], found["review-2"]["step"]) == (
        "build", "task_review", "plan_review")


# -- which piece of work a run is a round of

TASK_ENV = "SAKURA_FLOW_BUILD_TASK"


def write_plan(task):
    tasks = [{"id": "T1", "title": "Count the lines", "depends_on": [], "rows": [], "files": ["a.py"], "seam": False},
             {"id": "T2", "title": "Count the words", "depends_on": [], "rows": [], "files": ["b.py"], "seam": False}]
    task.plan_json.write_text(json.dumps({"tasks": tasks}))
    task.plan_md.write_text("# Plan\n")


def test_Sessions_Agent_ParallelBuildersAndAFixRound_AreTwoAgentsOneOfThemInTwoRounds(task, capsys):
    write_plan(task)
    add_run(task, "build-1", INIT, env={TASK_ENV: "T1"})
    add_run(task, "build-2", INIT, env={TASK_ENV: "T2"})
    add_run(task, "build-3", INIT, env={TASK_ENV: "T1"})

    found = rows(capsys)

    assert found["build-1"]["agent"] == found["build-3"]["agent"] != found["build-2"]["agent"]
    assert [found[name]["round"] for name in ("build-1", "build-2", "build-3")] == [1, 1, 2]
    assert [(found[name]["task_id"], found[name]["task_title"]) for name in ("build-1", "build-2")] == [
        ("T1", "Count the lines"), ("T2", "Count the words")]


def test_Sessions_Agent_APlannerThatWroteSpecContractAndPlan_IsThreeAgentsWithNoTask(task, capsys):
    write_plan(task)
    for name, phase in (("intake-1", "intake"), ("contract-1", "contract"), ("plan-1", "plan")):
        add_run(task, name, INIT, role="planner", phase=phase)

    found = rows(capsys)

    assert len({found[name]["agent"] for name in found}) == 3
    assert {(row["round"], row["task_id"], row["task_title"]) for row in found.values()} == {(1, "", "")}


def test_Sessions_Agent_AResumeSharesTheKeyOfItsRoot_AndCountsAsTheNextRound(task, capsys):
    write_plan(task)
    add_run(task, "build-1", INIT, env={TASK_ENV: "T1"})
    add_run(task, "build-1.2", INIT, env={TASK_ENV: "T1"}, resumes="build-1")
    add_run(task, "build-1.2.3", INIT, env={}, phase="resumed", resumes="build-1.2")

    found = rows(capsys)

    assert {found[name]["agent"] for name in found} == {found["build-1"]["agent"]}
    assert [found[name]["round"] for name in ("build-1", "build-1.2", "build-1.2.3")] == [1, 2, 3]
    assert found["build-1.2.3"]["task_id"] == "T1"


def test_Sessions_Agent_TheTaskReviewAndTheBuilderOfOneTask_AreNotOneAgent(task, capsys):
    write_plan(task)
    add_run(task, "build-1", INIT, env={TASK_ENV: "T1"})
    add_run(task, "review-1", INIT, role="code_reviewer", phase="review", env={TASK_ENV: "T1"})
    add_run(task, "review-2", INIT, role="code_reviewer", phase="review", env={TASK_ENV: "T1"}, step="final_review")

    found = rows(capsys)

    assert len({found[name]["agent"] for name in found}) == 3


def test_Sessions_Agent_ATaskThatIsNotInThePlan_HasNoTaskIdOrTitle(task, capsys):
    write_plan(task)
    add_run(task, "build-1", INIT, env={TASK_ENV: "FIX1"})

    found = rows(capsys)["build-1"]

    assert (found["task_id"], found["task_title"], found["round"]) == ("", "", 1)


def test_Sessions_Agent_NoPlanYet_NamesNoTask(task, capsys):
    add_run(task, "build-1", INIT, env={TASK_ENV: "T1"})

    found = rows(capsys)["build-1"]

    assert (found["task_id"], found["task_title"], found["round"]) == ("", "", 1)
