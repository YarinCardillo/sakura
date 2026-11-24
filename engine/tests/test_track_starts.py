import io
import os
import subprocess
import time
from concurrent.futures import ThreadPoolExecutor
from contextlib import nullcontext, redirect_stdout
from pathlib import Path
from threading import Event
from types import SimpleNamespace

import pytest
from sakura import actors, engine_home
from sakura.flow import cli, exits, registry, store, tasks
from sakura.tracks import TrackService
from sakura.tracks.domain import Refusal, RefusalCode
from sakura.tracks.starts import LaunchInputs, TrackStarts, task_id_for
from test_tracks_service import MASTER, OTHER_MASTER, PERSON, PHONE

INPUTS = LaunchInputs("Build a thing\n\nKeep the existing checks.")


@pytest.fixture
def world(repo):
    registry.remember(repo)
    service = TrackService()
    track = service.create("Delivery", "Ship", actor=PERSON, op_id="new-track")
    return service, TrackStarts(service), track, engine_home.project_id(repo)


def reserve(world, op_id="start", actor=PERSON, inputs=INPUTS):
    service, _, track, project = world
    return service.reserve_start(track.id, project, inputs, actor, op_id)


def materialize(reservation, *, pending=True, marker=True):
    task = store.TaskStore(Path(reservation.project.folder), reservation.task)
    state = {"task": reservation.task, "status": "running", "title": reservation.title, "ref": "text:fixture"}
    if marker:
        state[tasks.TRACK_START_KEY] = reservation.task
    if pending:
        state[tasks.PENDING_START_KEY] = True
    task.create(state, reservation.inputs.text.encode())
    return task


def execute(world, reservation, run, actor=PERSON):
    return world[1].execute(world[3], reservation.task, actor, run)


def child(work):
    pid = os.fork()
    if pid == 0:
        worked = False
        try:
            work()
            worked = True
        finally:
            os._exit(0 if worked else 1)
    return pid


def reap(pid, expected=0):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        done, status = os.waitpid(pid, os.WNOHANG)
        if done:
            assert os.waitstatus_to_exitcode(status) == expected
            return
        time.sleep(0.01)
    os.kill(pid, 9)
    os.waitpid(pid, 0)
    pytest.fail("Track worker did not exit")


@pytest.mark.parametrize("create_state", [False, True])
def test_crash_leaves_same_reserved_lane_for_retry(world, repo, create_state):
    reservation = reserve(world)

    def die(saved):
        if create_state:
            materialize(saved)
        os.kill(os.getpid(), 9)

    reap(child(lambda: execute(world, reservation, die)), -9)
    [row] = world[1].read_pending()
    assert (row.reservation.state, row.reservation.task) == ("interrupted", reservation.task)
    assert row.actions == (("retry",) if create_state else ("retry", "remove"))

    def recover(saved):
        task = store.TaskStore(repo, saved.task)
        assert task.exists() == create_state
        if not task.exists():
            materialize(saved)
        return exits.OK, task.task_id

    assert execute(world, reservation, recover) == (exits.OK, reservation.task)
    assert [task.task_id for task in store.task_stores(repo)] == [reservation.task]
    assert world[1].read_pending() == ()


def test_lost_success_reply_returns_committed_lane(world):
    reservation = reserve(world)

    def launch(saved):
        materialize(saved)
        return exits.OK, saved.task

    def disconnect(*args, **kwargs):
        execute(world, reservation, launch)
        raise OSError("connection lost after the child committed success")

    with pytest.raises(OSError):
        world[1].launch(reservation.op_id, PHONE, run=disconnect)
    assert world[1].launch(reservation.op_id, PERSON, run=lambda *a, **k: pytest.fail("relaunched")) == reservation.task


def test_driver_that_consumed_pending_start_is_recovered_without_spawn(world):
    reservation = reserve(world)

    def lose_result(saved):
        materialize(saved, pending=False)
        raise OSError("lost child output")

    with pytest.raises(OSError):
        execute(world, reservation, lose_result)
    assert execute(world, reservation, lambda _: pytest.fail("relaunched")) == (exits.OK, reservation.task)


def test_two_retry_processes_only_dispatch_once(world, tmp_path):
    reservation = reserve(world)
    invocations = tmp_path / "invocations"

    def launch(saved):
        with invocations.open("a") as stream:
            stream.write("launch\n")
        time.sleep(0.15)
        materialize(saved)
        return exits.OK, saved.task

    def retry():
        assert execute(world, reservation, launch) == (exits.OK, reservation.task)

    pids = [child(retry), child(retry)]
    for pid in pids:
        reap(pid)
    assert invocations.read_text() == "launch\n"


@pytest.mark.parametrize("launch_first", [False, True])
def test_remove_cannot_race_retry_into_an_orphan(world, launch_first):
    reservation = reserve(world)
    starts = world[1]

    if not launch_first:
        assert starts.remove(reservation.op_id, actor=PERSON, op_id="remove").state == "removed"
        assert starts.remove(reservation.op_id, actor=PHONE, op_id="remove").state == "removed"
        with pytest.raises(Refusal):
            execute(world, reservation, lambda _: pytest.fail("removed lane relaunched"))
        assert not world[0].read_all()[0].lanes
        return

    entered, release = Event(), Event()

    def launch(saved):
        entered.set()
        assert release.wait(5)
        materialize(saved)
        return exits.OK, saved.task

    with ThreadPoolExecutor(2) as callers:
        retry = callers.submit(execute, world, reservation, launch)
        try:
            assert entered.wait(5)
            [row] = starts.read_pending()
            assert "remove" not in row.actions
            remove = callers.submit(starts.remove, reservation.op_id, actor=PERSON, op_id="remove")
        finally:
            release.set()
        assert retry.result(timeout=5) == (exits.OK, reservation.task)
        with pytest.raises(Refusal):
            remove.result(timeout=5)
        assert world[0].read_all()[0].lanes[0].task == reservation.task


def test_close_racing_reserve_only_one_wins(world):
    service, _, track, _ = world
    with ThreadPoolExecutor(2) as callers:
        results = [callers.submit(reserve, world),
                   callers.submit(service.update, track.id, status="closed", actor=PERSON, op_id="close")]
    assert sum(isinstance(result.exception(), Refusal) for result in results) == 1
    [current] = service.read_all()
    assert bool(current.lanes) == (current.status == "open")


def test_reservation_refuses_changed_inputs_and_foreign_session(world):
    saved = reserve(world, actor=MASTER)
    assert reserve(world, actor=MASTER) == saved
    for actor, inputs, expected in ((MASTER, LaunchInputs("Other work"), RefusalCode.OPERATION_CONFLICT),
                                    (OTHER_MASTER, INPUTS, RefusalCode.FORBIDDEN),
                                    (PERSON, INPUTS, RefusalCode.FORBIDDEN)):
        with pytest.raises(Refusal) as refusal:
            reserve(world, actor=actor, inputs=inputs)
        assert refusal.value.code == expected
    with pytest.raises(Refusal):
        execute(world, saved, lambda _: pytest.fail("foreign session launched"), actor=OTHER_MASTER)


def test_existing_or_later_colliding_task_is_not_adopted(world, repo):
    collision = task_id_for("collision")
    store.TaskStore(repo, collision).create({"task": collision, "status": "running"})
    with pytest.raises(Refusal):
        reserve(world, op_id="collision")

    saved = reserve(world)
    materialize(saved, marker=False)
    with pytest.raises(Refusal):
        execute(world, saved, lambda _: pytest.fail("accepted a colliding lane"))
    with pytest.raises(Refusal):
        world[1].remove(saved.op_id, actor=PERSON, op_id="remove")


def test_created_reply_must_match_verified_durable_state(world):
    saved = reserve(world)
    with pytest.raises(Refusal):
        execute(world, saved, lambda reservation: (exits.OK, reservation.task))

    def wrong_id(reservation):
        materialize(reservation)
        return exits.OK, "some-lane-created-in-parallel"

    with pytest.raises(Refusal):
        execute(world, saved, wrong_id)
    [row] = world[1].read_pending()
    assert row.actions == ("retry",)


def test_missing_folder_disables_launch_and_preserves_verified_remove(world, repo):
    saved = reserve(world)
    repo.rename(repo.with_name("moved"))
    [row] = world[1].read_pending()
    assert row.actions == ("remove",)
    with pytest.raises(Refusal):
        world[1].launch(saved.op_id, PERSON, run=lambda *a, **k: pytest.fail("launched into missing folder"))
    world[1].remove(saved.op_id, actor=PERSON, op_id="remove")
    assert world[0].read_all()[0].projects[0].folder == str(repo)


def test_pending_start_cannot_be_moved_or_unlinked_through_ordinary_membership(world, repo):
    service, _, track, _ = world
    second = service.create("Other", "Ship", actor=PERSON, op_id="second")
    saved = reserve(world)
    materialize(saved)
    with pytest.raises(Refusal):
        service.link_lane(second.id, repo, saved.task, actor=PERSON, op_id="move")
    with pytest.raises(Refusal):
        service.unlink_lane(track.id, repo, saved.task, actor=PERSON, op_id="unlink")
    with pytest.raises(Refusal):
        service.update(track.id, status="closed", actor=PERSON, op_id="close")
    [member] = [lane for current in service.read_all() for lane in current.lanes]
    assert (member.track_id, member.task) == (track.id, saved.task)


def test_partial_task_with_only_inputs_can_retry_but_cannot_be_removed(world, repo):
    saved = reserve(world)
    task = store.TaskStore(repo, saved.task)
    with task.lock():
        task.input.write_text("partial")
    [row] = world[1].read_pending()
    assert row.actions == ("retry",)
    with pytest.raises(Refusal):
        world[1].remove(saved.op_id, actor=PERSON, op_id="remove")


def test_flow_launch_uses_frozen_args_and_recovers_opener(world, repo, monkeypatch, capsys):
    saved = reserve(world, actor=MASTER)
    monkeypatch.chdir(repo)
    monkeypatch.setenv(actors.TRUSTED_ENV, "1")
    monkeypatch.setattr(cli, "_tmux_surface", lambda cfg: None)
    monkeypatch.setattr(tasks, "check_codex_trust", lambda cfg, repo: None)
    monkeypatch.setattr(cli, "_detach_resume", lambda *a, **k: exits.OK)
    original = cli._make_task_repository

    def die(*args):
        raise OSError("lost before task repository")

    monkeypatch.setattr(cli, "_make_task_repository", die)
    args = ["start", f"--inbox-op={saved.task}", "--actor=app:owner", "--new", "--detach"]
    assert cli.main([*args, "--message=changed"]) == exits.USAGE
    with pytest.raises(OSError):
        cli.main(args)
    task = store.TaskStore(repo, saved.task)
    expected_branch = f"feat/flow-build-a-thing--{saved.task.removeprefix(tasks.TRACK_START_PREFIX)[:16]}"
    assert task.read_state()["branch"] == expected_branch
    task.ledger.unlink()
    monkeypatch.setattr(cli, "_make_task_repository", original)
    capsys.readouterr()
    assert cli.main(args) == exits.OK
    assert [line for line in capsys.readouterr().out.splitlines() if line.startswith("lane ")] == [f"lane {saved.task}"]
    state = task.read_state()
    assert state["branch"] == expected_branch
    assert state[tasks.TRACK_START_KEY] == saved.task and tasks.INBOX_START_KEY not in state
    assert state[tasks.PENDING_START_KEY] and task.input.read_text() == INPUTS.text + "\n"
    assert cli.lane_ceiling.opener(task) == MASTER
    assert world[1].launch(saved.op_id, PHONE, run=lambda *a, **k: subprocess.CompletedProcess(a, 1)) == saved.task

    other = reserve(world, op_id="another start", actor=MASTER)
    assert cli.main(["start", f"--inbox-op={other.task}", *args[2:]]) == exits.OK
    other_branch = store.TaskStore(repo, other.task).read_state()["branch"]
    assert other_branch.startswith("feat/flow-build-a-thing--") and other_branch != expected_branch


def test_launch_reads_created_id_from_the_normal_flow_command(world, repo, monkeypatch):
    def run(argv, *, cwd, env, **kwargs):
        assert cwd == str(repo) and env[actors.TRUSTED_ENV] == "1"
        monkeypatch.chdir(cwd)
        monkeypatch.setenv(actors.TRUSTED_ENV, env[actors.TRUSTED_ENV])
        output = io.StringIO()
        with redirect_stdout(output):
            code = cli.main(argv[2:])
        return subprocess.CompletedProcess(argv, code, output.getvalue(), "")

    saved = reserve(world, inputs=LaunchInputs(INPUTS.text, preset="solo"))
    monkeypatch.setattr(cli, "_tmux_surface", lambda cfg: None)
    monkeypatch.setattr(cli, "_detach_resume", lambda *a, **k: exits.OK)
    monkeypatch.setattr(tasks, "check_codex_trust", lambda *_: None)
    assert world[1].launch(saved.op_id, PHONE, run=run) == saved.task
    assert store.TaskStore(repo, saved.task).read_state()["preset"] == "solo"


def test_finished_lane_resume_cannot_revive_a_closed_track(world, repo, monkeypatch, capsys):
    from test_tracks_service import new_lane

    service, _, track, _ = world
    lane = new_lane(repo, status="report_ready")
    service.link_lane(track.id, repo, lane.task_id, actor=PERSON, op_id="link")
    service.update(track.id, status="closed", actor=PERSON, op_id="close")
    monkeypatch.chdir(repo)
    monkeypatch.setattr(cli.engine, "open_context", lambda *_: SimpleNamespace(store=lane, config=None))
    monkeypatch.setattr(tasks, "check_codex_trust", lambda *_: None)
    monkeypatch.setattr(cli.process, "hold_awake", lambda *_: nullcontext())
    assert cli.main(["resume", lane.task_id]) == exits.USAGE
    assert "Reopen this Track first" in capsys.readouterr().out
    assert lane.read_state()["status"] == "report_ready"


def test_close_racing_finished_lane_resume_observes_live_state(world, repo, monkeypatch):
    from test_tracks_service import new_lane

    service, _, track, _ = world
    lane = new_lane(repo, status="report_ready")
    service.link_lane(track.id, repo, lane.task_id, actor=PERSON, op_id="link")
    monkeypatch.setattr(cli.engine, "kill_stale_run", lambda *_: None)
    monkeypatch.setattr(cli.engine, "_spec_changed_after_approval", lambda *_: False)
    monkeypatch.setattr(cli.engine, "_reopen_stale_verification", lambda *_: None)
    entered, release = Event(), Event()
    set_status = cli.engine._set_status

    def hold(context, status, reason):
        entered.set()
        assert release.wait(5)
        set_status(context, status, reason)

    class BeganDriving(Exception):
        pass

    def stop_after_start(_):
        raise BeganDriving

    monkeypatch.setattr(cli.engine, "_set_status", hold)
    monkeypatch.setattr(cli.engine, "_block_on_error", lambda *_: None)
    monkeypatch.setattr(cli.engine.yield_person, "clear", stop_after_start)
    context = SimpleNamespace(store=lane, surface=SimpleNamespace(task_opened=lambda *_: None))
    with ThreadPoolExecutor(2) as callers:
        driving = callers.submit(cli.engine.advance, context)
        try:
            assert entered.wait(5)
            closing = callers.submit(service.update, track.id, status="closed", actor=PERSON, op_id="close")
        finally:
            release.set()
        with pytest.raises(BeganDriving):
            driving.result(timeout=5)
        with pytest.raises(Refusal) as refused:
            closing.result(timeout=5)
    assert refused.value.code == RefusalCode.OPEN_LANES
