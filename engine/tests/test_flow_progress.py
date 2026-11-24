"""Lane progress: done over total of the plan tasks and contract rows, from the engine's records only."""

from sakura.flow import checks, config, contract, final_head, plan, progress

HEAD = "a" * 40
OTHER_HEAD = "b" * 40
STEPS = config.PresetSteps(True, True, True)


def _row(row_id: str, passes: bool = True) -> contract.Row:
    return contract.Row(row_id, "journey", "step", "api", None, "true", False, passes)


def _task(task_id: str) -> plan.BuildTask:
    return plan.BuildTask(task_id, task_id, [], ["R1"], ["a.py"], False)


def _state(statuses: dict[str, dict], phase: str = "build", head: str = HEAD) -> dict:
    return {"phase": phase, "build": statuses, final_head.STATE_KEY: {"steps": [{"head": head}]}}


def _receipt(row: contract.Row, rows: list[contract.Row], head: str) -> dict:
    return {checks.row_key(row): {"exit": 0, "contract": contract.rows_digest(rows), "revision": head}}


def test_Progress_NoPlanYet_TotalZeroAndPhaseNamed():
    state = {"phase": "intake"}

    assert progress.of(state, [], [], STEPS, None) == progress.Progress(0, 0, "intake")


def test_Progress_OneOfTwoTasksMergedOneOfThreeRowsProven_TwoOfFive():
    rows = [_row("R1"), _row("R2"), _row("R3", passes=False)]
    state = _state({"T1": {"status": "merged"}, "T2": {"status": "building"}})
    state[checks.EVALUATOR_RECEIPTS_KEY] = _receipt(rows[0], rows, HEAD)

    result = progress.of(state, [_task("T1"), _task("T2")], rows, STEPS, None)

    assert result == progress.Progress(2, 5, "build")


def test_Progress_RowGreenWithProofAtAnotherHead_NotDone():
    rows = [_row("R1")]
    state = _state({}, head=HEAD)
    state[checks.EVALUATOR_RECEIPTS_KEY] = _receipt(rows[0], rows, OTHER_HEAD)

    assert progress.of(state, [], rows, STEPS, None).done == 0


def test_Progress_FixRoundClearsRows_DoneGoesDown():
    rows = [_row("R1")]
    state = _state({})
    state[checks.EVALUATOR_RECEIPTS_KEY] = _receipt(rows[0], rows, HEAD)
    before = progress.of(state, [], rows, STEPS, None)

    cleared = [_row("R1", passes=False)]
    after = progress.of(state, [], cleared, STEPS, None)

    assert (before.done, after.done) == (1, 0)


def test_Progress_TaskApprovedAsNoChange_CountsDone():
    state = _state({"T1": {"status": "approved", "no_change": "already does it"},
                    "T2": {"status": "approved", "no_change": ""}})

    result = progress.of(state, [_task("T1"), _task("T2")], [], STEPS, None)

    assert result == progress.Progress(1, 2, "build")
