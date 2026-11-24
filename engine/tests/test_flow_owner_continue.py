"""A lane blocked only on owner rows continues once the last one is marked: from `flow mark --continue --detach` and
from a cockpit Resume that is accepted for it."""

import pytest

from sakura.cockpit import actions, items
from sakura.flow import ask_words, cli, exits, owner_marks, resume, waits
from test_flow_provers import STEPS_ON, _row, _saved

BLOCKED = "blocked"
ACTOR = "cli:tester"


def owner_task(repo, row_ids=("R1", "R2"), status=BLOCKED):
    rows = [_row(row_id, "owner", f"look at {row_id}") for row_id in row_ids]
    ctx = _saved(repo, rows)
    reason = owner_marks.blocked_reason("demo", rows)
    ctx.store.update_state(lambda state: state.update(phase="pr", status=status, reason=reason))
    return ctx.store


@pytest.fixture
def detached(monkeypatch, repo):
    """The detached resumes `flow mark` asked for."""
    monkeypatch.chdir(repo)
    calls = []
    monkeypatch.setattr(cli, "_detach_resume", lambda cfg, task_store, actor: calls.append(task_store.task_id) or exits.OK)
    return calls


def mark(*words):
    return cli.main(["mark", "demo", *words, f"--actor={ACTOR}"])


def test_Mark_ContinueDetach_LastOpenRow_ResumesDetached(repo, detached):
    owner_task(repo)

    assert mark("R1", "ok", "--continue", "--detach") == exits.OK
    assert detached == []
    assert mark("R2", "ok", "--continue", "--detach") == exits.OK
    assert detached == ["demo"]


def test_Mark_ContinueDetach_LastOpenRow_NoResumeHint(repo, detached, capsys):
    owner_task(repo, ("R1",))

    assert mark("R1", "ok", "--continue", "--detach") == exits.OK

    out = capsys.readouterr().out
    assert "row R1 marked ok" in out and "run sakura flow resume" not in out


def test_Mark_WithoutContinue_PrintsResumeHint(repo, detached, capsys):
    owner_task(repo, ("R1",))

    assert mark("R1", "ok") == exits.OK

    assert "row R1 marked ok; run sakura flow resume demo" in capsys.readouterr().out


def test_Mark_ContinueDetach_RowMarkedKo_StaysWaiting(repo, detached):
    owner_task(repo, ("R1",))

    assert mark("R1", "ko", "--continue", "--detach") == exits.OK

    assert detached == []


def test_Mark_ContinueDetach_TaskNotBlockedOnOwnerRows_DoesNotResume(repo, detached):
    owner_task(repo, ("R1",), status="running")

    assert mark("R1", "ok", "--continue", "--detach") == exits.OK

    assert detached == []


def test_Mark_WithoutContinue_NeverResumes(repo, detached):
    owner_task(repo, ("R1",))

    assert mark("R1", "ok") == exits.OK

    assert detached == []


def test_Mark_DetachWithoutContinue_Refused(repo, detached, capsys):
    owner_task(repo, ("R1",))

    assert mark("R1", "ok", "--detach") == exits.USAGE

    assert "--detach needs --continue" in capsys.readouterr().out
    assert detached == []


def blocked_item(task):
    return items.flow_item(task, lambda key: None)


def test_Item_AllOwnerRowsMarked_IsWaitingForResumeAndAcceptsIt(repo):
    task = owner_task(repo, ("R1",))
    assert blocked_item(task).agent_state == "blocked"
    owner_marks.mark(task, STEPS_ON, "R1", "ok", "", ACTOR)

    item = blocked_item(task)

    assert item.agent_state == "waiting" and item.next_command == "sakura flow resume demo"
    assert actions.ACTIONS["R"].build(item, {}).argv[:5] == [str(actions.PROGRAM), "flow", "resume", "--detach",
                                                            f"--stopped-revision={item.resume_revision}"]


def test_Item_OneOfTwoOwnerRowsMarked_StaysBlockedAndRefusesResume(repo):
    task = owner_task(repo)
    owner_marks.mark(task, STEPS_ON, "R1", "ok", "", ACTOR)

    item = blocked_item(task)

    assert item.agent_state == "blocked"
    with pytest.raises(actions.ActionRefused):
        actions.ACTIONS["R"].build(item, {})


def test_Item_BlockedForAReasonARetryCannotClear_RefusesResumeAndOffersWhatHelps(repo):
    task = owner_task(repo, ("R1",))
    owner_marks.mark(task, STEPS_ON, "R1", "ok", "", ACTOR)
    task.update_state(lambda state: state.update(reason="pull request not merged"))
    item = blocked_item(task)

    with pytest.raises(actions.ActionRefused, match="not merged|retry"):
        actions.ACTIONS["R"].build(item, {})
    offered = [action.key for action in actions.applicable(item)]
    assert "R" not in offered and "s" in offered


def test_Item_BlockedForAReasonARetryCanClear_OffersResumeAtTheCurrentRevision(repo):
    task = owner_task(repo, ("R1",))
    owner_marks.mark(task, STEPS_ON, "R1", "ok", "", ACTOR)
    task.update_state(lambda state: state.update(reason="push stalled; see the log"))
    item = blocked_item(task)

    assert "R" in [action.key for action in actions.applicable(item)]
    assert f"--stopped-revision={item.resume_revision}" in actions.ACTIONS["R"].build(item, {}).argv


def test_Item_OwnerRowsOpen_OffersMarkNotResumeUntilEveryRowIsMarked(repo):
    from sakura.cockpit import wait_actions

    task = owner_task(repo)
    item = blocked_item(task)
    assert "R" not in [action.key for action in actions.applicable(item)]
    open_rows = [wait for wait in waits.collect([repo]) if wait.kind == waits.OWNER_ROW]
    assert open_rows and all(wait_actions.allows(wait_actions.target_of(wait), "k") for wait in open_rows)
    owner_marks.mark(task, STEPS_ON, "R1", "ok", "", ACTOR)
    owner_marks.mark(task, STEPS_ON, "R2", "ok", "", ACTOR)
    marked = blocked_item(task)
    assert "R" in [action.key for action in actions.applicable(marked)]


def test_Admission_OwnerRowsMarked_AcceptsTheCurrentRevisionOnly(repo):
    task = owner_task(repo, ("R1",))
    owner_marks.mark(task, STEPS_ON, "R1", "ok", "", ACTOR)
    state = task.read_state()

    resume.require_stopped(state, resume.revision(state), owner_marked=True)
    with pytest.raises(Exception, match="has changed"):
        resume.require_stopped(state, "stale", owner_marked=True)
    with pytest.raises(Exception, match="has changed"):
        resume.require_stopped(state, resume.revision(state))


def test_Wait_BlockedOnOwnerRows_ReasonFollowsTheMarks(repo):
    task = owner_task(repo)

    def blocked_detail():
        return next(wait.detail for wait in waits.collect([repo]) if wait.kind == waits.BLOCKED)

    assert blocked_detail() == "Waiting for you to try 2 things yourself."
    owner_marks.mark(task, STEPS_ON, "R1", "ok", "", ACTOR)
    assert blocked_detail() == "Waiting for you to try 1 thing yourself."
    owner_marks.mark(task, STEPS_ON, "R2", "ok", "", ACTOR)
    assert blocked_detail() == ask_words.ALL_TRIED_DETAIL
