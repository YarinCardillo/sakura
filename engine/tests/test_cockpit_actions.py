import dataclasses
import json
import os
import stat
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest
from sakura import engine_home
from sakura.actors import Actor
from sakura.cockpit import actions
from sakura.cockpit.items import GroupRow, Item
from sakura.cockpit.project_actions import ProjectTarget
from sakura.cockpit.wait_actions import WaitTarget
from sakura.flow import inbox, store, waits
from test_cockpit_snapshot import create_task, other_repo

PROGRAM = str(actions.PROGRAM)
OWNER = Actor("overlay", "owner@example.com", "nPHONE1", "Owner's phone")
ACTOR_OPTION = "--actor=overlay:owner@example.com:nPHONE1"


def flow_item(repo, task="t1"):
    return Item("flow", ("flow", str(repo), task), Path(repo), None, "feat/x", "", "up", "waiting", "", {}, [], None, "")


def lane_item(repo, group="proc", name="auth"):
    return Item("lane", ("lane", group, name), Path(repo), None, "feat/auth", "", "up", "waiting", "", {}, [], None, "")


REPO_PATH = Path("/work/repo")


def group_row(repo, name="proc"):
    return GroupRow(name, Path(repo), "proc-session", 0, "", "")


def wait_target(kind, key, task="t1", repo=REPO_PATH):
    """The target of a wait of `task`; `key` is the gate or the row the wait names."""
    identity = json.dumps(["wait", str(repo), task, kind, key], separators=(",", ":"))
    return WaitTarget(waits.Wait(identity, kind, str(repo), "repo", task, "title", "detail", None))


class Spawns:
    """A subprocess.run double: records every call and answers with a fixed result."""

    def __init__(self, stdout="done\n", stderr="", returncode=0):
        self.calls = []
        self._answer = (stdout, stderr, returncode)

    def __call__(self, argv, **kwargs):
        self.calls.append((argv, kwargs))
        stdout, stderr, returncode = self._answer
        return subprocess.CompletedProcess(argv, returncode, stdout, stderr)


def log_rows(path):
    return [json.loads(line) for line in path.read_text().splitlines()]


@pytest.fixture
def log_path(tmp_path):
    return tmp_path / "cockpit" / "actions.jsonl"


def invoke(key, target, inputs=None, spawns=None, actor=OWNER, log=None):
    spawns = spawns or Spawns()
    return actions.run(key, target, inputs or {}, actor, run=spawns, log_path=log), spawns


REPO = Path("/work/repo")
FLOW, LANE, GROUP = flow_item(REPO), lane_item(REPO), group_row(REPO)

TABLE = [
    ("d", LANE, {}, ["lane", "done", "--group=proc", "--", "auth"]),
    ("m", FLOW, {"role": "builder", "model": "claude:opus"}, ["flow", "role", "--", "t1", "builder", "claude:opus"]),
    ("m", LANE, {"model": "opus"}, ["lane", "model", "--group=proc", "--", "auth", "opus"]),
    ("D", FLOW, {"step": "strong"}, ["flow", "dial", "--", "t1", "strong"]),
    ("a", FLOW, {}, ["flow", "approve", "--continue", "--detach", "--", "t1", "spec"]),
    ("s", FLOW, {}, ["flow", "stop", "--", "t1"]),
    ("w", FLOW, {"text": "prefer short names"}, ["flow", "steer", "--", "t1", "prefer short names"]),
    ("w", LANE, {"text": "run the tests"}, ["lane", "say", "--group=proc", "--", "auth", "run the tests"]),
    ("L", ProjectTarget(REPO), {"text": "add --help"}, ["flow", "start", "--detach", "--new", "--message=add --help", "--"]),
    ("L", ProjectTarget(REPO), {"text": "add --help", "preset": "lean", "level": "fast"},
     ["flow", "start", "--detach", "--new", "--message=add --help", "--preset=lean", "--relax=preset", "--set=knobs.lane_pace=fast", "--"]),
    ("L", ProjectTarget(REPO), {"text": "add --help", "level": "slow"},
     ["flow", "start", "--detach", "--new", "--message=add --help", "--set=knobs.lane_pace=slow", "--"]),
]


@pytest.mark.parametrize(("key", "target", "inputs", "tail"), TABLE)
def test_each_action_builds_exact_invocation(key, target, inputs, tail):
    invocation = actions.ACTIONS[key].build(target, inputs)
    assert invocation.argv == [PROGRAM, *tail]
    assert invocation.cwd == REPO


@pytest.mark.parametrize(("key", "target", "inputs", "tail"), TABLE)
def test_run_spawns_the_built_argv_with_the_actor_before_the_separator(key, target, inputs, tail, log_path):
    _, spawns = invoke(key, target, inputs, log=log_path)
    (argv, kwargs), = spawns.calls
    separator = argv.index("--")
    assert argv[separator - 1] == ACTOR_OPTION
    assert [part for part in argv if part != ACTOR_OPTION] == [PROGRAM, *tail]
    assert kwargs["cwd"] == REPO and "shell" not in kwargs


def test_spawn_strips_tmux_variables_from_the_child_environment(monkeypatch, log_path):
    monkeypatch.setenv("TMUX", "/tmp/owner-socket,1,0")
    monkeypatch.setenv("TMUX_PANE", "%3")
    monkeypatch.setenv("TMUX_TMPDIR", "/tmp/owner-tmux")
    monkeypatch.setenv("KEEP_ME", "yes")
    _, spawns = invoke("D", FLOW, {"step": "strong"}, log=log_path)
    child_env = spawns.calls[0][1]["env"]
    assert "TMUX" not in child_env and "TMUX_PANE" not in child_env
    assert child_env["TMUX_TMPDIR"] == "/tmp/owner-tmux"  # the lane command it runs must still reach the person's server
    assert child_env["KEEP_ME"] == "yes"


def test_jumping_to_a_window_is_not_an_action(log_path):
    assert "enter" not in actions.ACTIONS
    for target in (FLOW, LANE):
        with pytest.raises(actions.ActionRefused, match="unknown action"):
            invoke("enter", target, log=log_path)


STOPPED = dataclasses.replace(FLOW, agent_state="waiting", next_command="sakura flow resume t1")


def test_resume_runs_a_detached_resume_of_the_stopped_flow(log_path):
    invocation = actions.ACTIONS["R"].build(STOPPED, {})
    assert invocation.argv == [PROGRAM, "flow", "resume", "--detach", "--stopped-revision=", "--", "t1"] and invocation.cwd == REPO
    _, spawns = invoke("R", STOPPED, log=log_path)
    (argv, _), = spawns.calls
    assert argv[argv.index("--") - 1] == ACTOR_OPTION


@pytest.mark.parametrize(("state", "next_command", "reason"), [
    ("working", "sakura flow attach t1", "working"),
    ("gate", "sakura flow approve t1 spec --continue", "waits for your approval"),
    ("blocked", "sakura flow resume t1", "blocked or failed"),
    ("waiting", "sakura flow report t1", "finished"),
    ("waiting", "", "finished"),
])
def test_resume_refuses_a_flow_that_is_not_stopped_and_spawns_nothing(state, next_command, reason, log_path):
    item = dataclasses.replace(FLOW, agent_state=state, next_command=next_command)
    spawns = Spawns()
    with pytest.raises(actions.ActionRefused, match=f"cannot resume t1: .*{reason}"):
        invoke("R", item, spawns=spawns, log=log_path)
    assert spawns.calls == []
    assert [row["outcome"] for row in log_rows(log_path)] == ["refused"]


def test_resume_does_not_apply_to_a_lane_or_a_group(log_path):
    for target in (LANE, GROUP):
        with pytest.raises(actions.ActionRefused, match="does not apply"):
            invoke("R", target, log=log_path)


def test_resume_reports_only_proven_prelaunch_exits_as_refusals():
    busy = actions.ActionResult(actions.Invocation([], REPO), 6, "engine already running (pid 4242)\n")
    assert actions.refusal("R", busy) == "engine already running (pid 4242)"
    failed = actions.ActionResult(actions.Invocation([], REPO), 4, "tmux failed at /home/owner/.tmux/sock\n")
    assert actions.refusal("R", failed) is None
    assert "/Users" not in actions.client_result("R", failed).output
    assert actions.refusal("R", dataclasses.replace(busy, exit_code=0)) is None


def test_resume_gives_a_client_no_raw_output_on_success():
    done = actions.ActionResult(actions.Invocation([], REPO), 0, "detached: sess; sakura flow attach t1 /home/x\n")
    assert actions.client_result("R", done).output == ""


def test_approve_targets_the_gate_the_item_waits_at(log_path):
    import dataclasses
    waiting = dataclasses.replace(FLOW, next_command="sakura flow approve t1 opus:T2 --continue")
    _, spawns = invoke("a", waiting, log=log_path)
    assert spawns.calls[0][0][-2:] == ["t1", "opus:T2"]


def test_approve_targets_the_budget_gate(log_path):
    import dataclasses
    waiting = dataclasses.replace(FLOW, next_command="sakura flow approve t1 budget --continue")
    _, spawns = invoke("a", waiting, log=log_path)
    assert spawns.calls[0][0][-2:] == ["t1", "budget"]


def test_approve_targets_the_budget_wait_gate(log_path):
    import dataclasses
    waiting = dataclasses.replace(FLOW, next_command="sakura flow approve t1 budget-wait --continue")
    _, spawns = invoke("a", waiting, log=log_path)
    assert spawns.calls[0][0][-2:] == ["t1", "budget-wait"]


def test_only_designated_actions_require_confirmation():
    assert {key for key, action in actions.ACTIONS.items() if action.confirm} == {
        "a", "R", "s", "C", "T", "d", "n", "X", "y", "N", "H",
        "connectoradd", "connectoredit", "connectordisconnect", "connectorreplace", "scheduleddelete",
    }


@pytest.mark.parametrize(("key", "target", "inputs"), [
    ("x", FLOW, {}),
    ("", FLOW, {}),
    ("o", FLOW, {"name": "auth"}),
    ("D", LANE, {"step": "strong"}),
    ("d", GROUP, {}),
    ("s", LANE, {}),
    ("a", GROUP, {}),
    ("o", GROUP, {"name": "auth"}),
    ("D", FLOW, {"step": "strong", "extra": "x"}),
    ("D", FLOW, {"step": "../strong"}),
    ("D", FLOW, {"step": 3}),
    ("m", FLOW, {"model": "opus"}),
    ("m", FLOW, {"role": "builder; rm", "model": "claude:opus"}),
    ("m", FLOW, {"role": "builder", "model": "claude:opus\n"}),
    ("m", LANE, {"role": "builder", "model": "opus"}),
    ("m", LANE, {"model": "opus; rm -rf"}),
    ("m", LANE, {"model": "x" * 81}),
    ("w", FLOW, {"text": ""}),
    ("w", FLOW, {"text": "  \n"}),
    ("w", LANE, {"text": "esc \x1b[31m"}),
    ("w", LANE, {"text": "nul \x00"}),
    ("w", FLOW, {"text": "x" * (actions.MAX_TEXT + 1)}),
])
def test_forged_or_wrong_kind_refused_before_spawn(key, target, inputs, log_path):
    spawns = Spawns()
    with pytest.raises(actions.ActionRefused):
        invoke(key, target, inputs, spawns, log=log_path)
    assert spawns.calls == []
    (row,) = log_rows(log_path)
    expected = key if key in actions.ACTIONS else "unknown"
    assert row["key"] == expected and row["outcome"] == "refused" and row["reason"]


def test_an_invalid_actor_is_refused_before_spawn(log_path):
    spawns = Spawns()
    with pytest.raises(actions.ActionRefused, match="actor"):
        invoke("s", FLOW, spawns=spawns, actor=Actor("overlay", "bad login"), log=log_path)
    assert spawns.calls == []


def test_model_needs_depend_on_target():
    model = actions.ACTIONS["m"]
    assert model.needs(FLOW) == ("role", "model")
    assert model.needs(LANE) == ("model",)
    assert actions.ACTIONS["d"].needs(LANE) == ()


def test_text_with_shell_syntax_stays_one_argument(log_path):
    text = 'a; rm -rf / && echo "$(id)" `x` \'q\'\nsecond line\t--actor=evil'
    for key, target, tail in (("w", FLOW, ["flow", "steer"]), ("w", LANE, ["lane", "say"])):
        _, spawns = invoke(key, target, {"text": text}, log=log_path)
        (argv, kwargs), = spawns.calls
        assert argv[-1] == text and argv.index("--") < len(argv) - 1
        assert isinstance(argv, list) and not kwargs.get("shell")


def test_hostile_output_stripped(log_path):
    hostile = "ok\x1b]52;c;ZXZpbA==\x07\x1b[31mred\x1b[0m\x1b]2;title\x1b\\ end\x9b1m\x00"
    result, _ = invoke("s", FLOW, spawns=Spawns(stdout=hostile, stderr="\x1b[2Jerr\n"), log=log_path)
    assert "\x1b" not in result.output and "\x00" not in result.output and "\x9b" not in result.output
    assert "red" in result.output and "err" in result.output
    assert "\x1b" not in log_path.read_text()


def test_nonzero_exit_is_reported_and_logged_never_retried(log_path):
    result, spawns = invoke("s", FLOW, spawns=Spawns(stdout="blocked\n", returncode=3), log=log_path)
    assert result.exit_code == 3 and "blocked" in result.output and len(spawns.calls) == 1
    assert log_rows(log_path)[0]["outcome"] == "failed" and log_rows(log_path)[0]["exit_code"] == 3


def test_spawn_failure_and_timeout_are_failed_results(log_path):
    def missing(argv, **kwargs):
        raise FileNotFoundError("no such program \x1b[31m")

    def slow(argv, **kwargs):
        raise subprocess.TimeoutExpired(argv, kwargs["timeout"])

    for double in (missing, slow):
        result = actions.run("s", FLOW, {}, OWNER, run=double, log_path=log_path)
        assert result.exit_code != 0 and result.output and "\x1b" not in result.output
    assert [row["outcome"] for row in log_rows(log_path)] == ["failed", "failed"]


def test_a_failing_audit_append_after_the_command_ran_is_a_done_result_with_an_audit_note(tmp_path, capsys):
    blocked = tmp_path / "actions.jsonl"
    blocked.mkdir()
    result, spawns = invoke("w", FLOW, {"text": "hello"}, log=blocked)
    assert result.exit_code == 0 and len(spawns.calls) == 1
    assert result.audit.startswith("not written: ") and "Is a directory" in result.audit
    assert "cannot write" in capsys.readouterr().err


def test_every_attempt_is_logged_with_the_full_actor_and_stripped(log_path):
    actor = Actor("overlay", "owner@example.com", "nPHONE1", "Phone \x1b]2;x\x07\n[forged]")
    invoke("D", FLOW, {"step": "strong"}, actor=actor, log=log_path)
    (row,) = log_rows(log_path)
    assert (row["actor"]["source"], row["actor"]["login"], row["actor"]["node_id"]) == (
        "overlay", "owner@example.com", "nPHONE1")
    assert "Phone" in row["actor"]["device"] and "\x1b" not in row["actor"]["device"]
    assert len(log_path.read_text().splitlines()) == 1
    assert row["outcome"] == "done" and row["exit_code"] == 0
    assert row["target"]["kind"] == "flow" and "argv" not in row
    assert row["time"]


def test_default_log_is_under_the_home_the_process_sees_without_an_engine_home(tmp_path, monkeypatch):
    home = tmp_path / "other-home"
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.delenv("SAKURA_ENGINE_HOME")
    invoke("s", FLOW)
    assert (home / ".local/state/sakura/cockpit/actions.jsonl").is_file()


def test_default_log_is_in_the_state_folder_of_the_engine_home(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path / "other-home"))
    invoke("s", FLOW)
    assert (engine_home.root() / "state" / "cockpit" / "actions.jsonl").is_file()
    assert not (tmp_path / "other-home").exists()


def test_same_task_two_repos_only_selected_changes(tmp_path, repo):
    first, second = repo, other_repo(tmp_path)
    for project in (first, second):
        create_task(project, "shared-id", status="running")
    result = actions.run("D", flow_item(second, "shared-id"), {"step": "power"}, OWNER,
                         log_path=tmp_path / "actions.jsonl")
    assert result.exit_code == 0, result.output
    pending = {repo: [command for _, command in inbox.pending(store.TaskStore(repo, "shared-id"))]
               for repo in (first, second)}
    assert pending[first] == []
    assert [(c["kind"], c["value"], c["actor"]) for c in pending[second]] == [
        ("dial", "power", "overlay:owner@example.com:nPHONE1")]


def test_same_lane_two_groups_only_selected_changes(log_path):
    _, spawns = invoke("m", lane_item(REPO, group="beta", name="auth"), {"model": "opus"}, log=log_path)
    (argv, _), = spawns.calls
    assert "--group=beta" in argv and "--group=proc" not in argv and argv[-2:] == ["auth", "opus"]


@pytest.mark.parametrize(("key", "target", "inputs", "tail"), TABLE)
def test_every_built_command_parses_to_the_selected_target(key, target, inputs, tail, log_path):
    from sakura.flow import cli as flow_cli
    from sakura.lanes import cli as lane_cli

    invocation = actions.run(key, target, inputs, OWNER, run=Spawns(), log_path=log_path).invocation
    words = invocation.argv[1:]
    parser = flow_cli._parser() if words[0] == "flow" else lane_cli._parser()
    parsed = parser.parse_args(words[1:])
    assert parsed.command == words[1]
    if words[0] == "lane":
        assert parsed.group == (target.name if isinstance(target, GroupRow) else target.key[1])
    if "text" in inputs:
        assert (parsed.message if key == "L" else parsed.text) == inputs["text"]
    assert getattr(parsed, "actor", ACTOR_OPTION[8:]) == ACTOR_OPTION[8:]


# -- review fixes ------------------------------------------------------------------------------


def test_refusal_survives_a_log_write_failure_and_reports_it_on_stderr(tmp_path, capsys):
    blocked = tmp_path / "cockpit"
    blocked.write_text("a file where the log directory should be")
    with pytest.raises(actions.ActionRefused, match="unknown action"):
        actions.run("x", FLOW, {}, OWNER, run=Spawns(), log_path=blocked / "actions.jsonl")
    error = capsys.readouterr().err
    assert "cannot write" in error and "actions.jsonl" in error


def test_action_log_is_private(tmp_path):
    log = tmp_path / "state" / "cockpit" / "actions.jsonl"
    invoke("s", FLOW, log=log)
    assert stat.S_IMODE(log.stat().st_mode) == 0o600
    assert stat.S_IMODE(log.parent.stat().st_mode) == 0o700


@pytest.mark.parametrize("character", ["\u202e", "\u200b", "\u2028", "\u2029", "\ufeff"])
def test_invisible_format_and_separator_characters_are_refused_in_one_line_inputs(character, log_path):
    spawns = Spawns()
    with pytest.raises(actions.ActionRefused, match="one line"):
        invoke("m", FLOW, {"role": "builder", "model": f"claude:opus{character}"}, spawns, log=log_path)
    assert spawns.calls == []


# -- budgets and process groups ----------------------------------------------------------------


def test_every_action_keeps_the_default_budget(log_path):
    assert actions.ACTION_TIMEOUT_SECONDS == 120
    for key, target, inputs, _ in TABLE:
        _, spawns = invoke(key, target, inputs, log=log_path)
        assert spawns.calls[0][1]["timeout"] == actions.ACTION_TIMEOUT_SECONDS, key


def test_the_open_lane_action_is_shelved_with_the_tmux_lanes():
    assert "Open lane" not in [action.label for action in actions.ACTIONS.values()]


GRANDCHILD_PARENT = textwrap.dedent("""
    import os, subprocess, sys, time
    grandchild = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(300)"])
    open(sys.argv[1], "w").write(f"{os.getpid()} {grandchild.pid}")
    time.sleep(300)
""")


def start_group(tmp_path, timeout):
    """Run a parent that spawns a long-lived grandchild through the action runner until it times out; return both pids."""
    pids = tmp_path / "pids"
    argv = [sys.executable, "-c", GRANDCHILD_PARENT, str(pids)]
    with pytest.raises(subprocess.TimeoutExpired):
        actions.run_in_own_group(argv, cwd=tmp_path, env=os.environ.copy(), stdin=subprocess.DEVNULL, timeout=timeout)
    parent, grandchild = (int(pid) for pid in pids.read_text().split())
    return parent, grandchild


def test_a_timed_out_action_leaves_no_live_process_of_its_group(tmp_path):
    parent, grandchild = start_group(tmp_path, timeout=1)
    with pytest.raises(ProcessLookupError):
        os.killpg(parent, 0)
    with pytest.raises(ProcessLookupError):
        os.kill(grandchild, 0)


def test_a_retry_after_a_timeout_starts_with_the_first_groups_processes_gone(tmp_path):
    first_parent, first_grandchild = start_group(tmp_path, timeout=1)
    second_parent, second_grandchild = start_group(tmp_path, timeout=1)
    for pid in (first_parent, first_grandchild, second_parent, second_grandchild):
        with pytest.raises(ProcessLookupError):
            os.kill(pid, 0)


def test_a_timed_out_action_is_a_failed_result_after_its_group_is_reaped(tmp_path):
    pids = tmp_path / "pids"
    invocation = actions.Invocation([sys.executable, "-c", GRANDCHILD_PARENT, str(pids)], tmp_path)
    result = actions._spawn(invocation, actions.run_in_own_group, timeout=1)
    parent = int(pids.read_text().split()[0])
    assert result.exit_code != 0 and "timed out" in result.output
    with pytest.raises(ProcessLookupError):
        os.killpg(parent, 0)


def test_the_runner_returns_the_output_and_code_of_a_command_that_finishes(tmp_path):
    done = actions.run_in_own_group([sys.executable, "-c", "import sys; print('out'); sys.stderr.write('err'); sys.exit(3)"],
                                    cwd=tmp_path, env=os.environ.copy(), stdin=subprocess.DEVNULL, timeout=30)
    assert (done.returncode, done.stdout, done.stderr) == (3, "out\n", "err")


def test_Act_MarkOk_RunsFlowMarkWithActor(log_path):
    owner_row = wait_target("owner_row", "R1")

    _, spawns = invoke("k", owner_row, {"verdict": "ok", "note": "looks right"}, log=log_path)

    (argv, kwargs), = spawns.calls
    assert argv == [PROGRAM, "flow", "mark", "--continue", "--detach", "--note=looks right", ACTOR_OPTION, "--", "t1", "R1",
                    "ok"]
    assert kwargs["cwd"] == REPO


def test_Act_MarkWithoutNote_HasNoNoteOption(log_path):
    _, spawns = invoke("k", wait_target("owner_row", "R1"), {"verdict": "ko"}, log=log_path)

    assert spawns.calls[0][0] == [PROGRAM, "flow", "mark", "--continue", "--detach", ACTOR_OPTION, "--", "t1", "R1", "ko"]


@pytest.mark.parametrize(("target", "inputs"), [
    (wait_target("approval", "checks"), {"verdict": "ok"}),
    (wait_target("blocked", "blocked"), {"verdict": "ok"}),
    (FLOW, {"verdict": "ok"}),
    (wait_target("owner_row", "R1"), {"verdict": "maybe"}),
    (wait_target("owner_row", "R1"), {}),
    (wait_target("owner_row", "R1"), {"verdict": "ok", "note": "a\nb"}),
])
def test_Act_MarkOnAnythingButAnOwnerRowWithAVerdict_RefusedBeforeSpawn(target, inputs, log_path):
    spawns = Spawns()

    with pytest.raises(actions.ActionRefused):
        invoke("k", target, inputs, spawns, log=log_path)

    assert spawns.calls == []


def test_Act_ApproveWait_RunsTheGateTheWaitNames(log_path):
    _, spawns = invoke("a", wait_target("approval", "opus:T2"), log=log_path)

    assert spawns.calls[0][0][-2:] == ["t1", "opus:T2"]


def test_Act_ApproveRememberOnChecksWait_AddsRemember(log_path):
    _, spawns = invoke("a", wait_target("approval", "checks"), {"remember": "yes"}, log=log_path)

    assert spawns.calls[0][0] == [PROGRAM, "flow", "approve", "--continue", "--detach", "--remember", ACTOR_OPTION,
                                  "--", "t1", "checks"]


@pytest.mark.parametrize("target", [wait_target("approval", "spec"), FLOW, wait_target("approval", "budget")])
def test_Act_ApproveRememberOnSpecGate_Refused(target, log_path):
    spawns = Spawns()

    with pytest.raises(actions.ActionRefused, match="checks gate"):
        invoke("a", target, {"remember": "yes"}, spawns, log=log_path)

    assert spawns.calls == []


@pytest.mark.parametrize("target", [wait_target("owner_row", "R1"), wait_target("blocked", "blocked")])
def test_Act_ApproveOnAWaitThatIsNoApproval_Refused(target, log_path):
    with pytest.raises(actions.ActionRefused, match="approval"):
        invoke("a", target, log=log_path)


def test_Act_WaitKinds_OnlyTheirOwnActionsApply():
    keys = lambda target: [action.key for action in actions.applicable(target)]

    assert keys(wait_target("approval", "spec")) == ["a", "read", "unread", "snooze"]
    assert keys(wait_target("owner_row", "R1")) == ["k", "read", "unread", "snooze"]
    assert keys(wait_target("blocked", "blocked")) == ["read", "unread", "snooze"]


def test_StartLane_OnlyAProjectTakesIt():
    project = ProjectTarget(REPO)
    assert [action.key for action in actions.applicable(project)] == ["L", "W"]
    for target in (FLOW, LANE, GROUP):
        assert "L" not in [action.key for action in actions.applicable(target)]
        with pytest.raises(actions.ActionRefused):
            actions.plan("L", target, {"text": "x"}, OWNER)


@pytest.mark.parametrize("text", ["", "x" * 8001, "a\x00b"])
def test_StartLane_TextRefused_NothingPlanned(text):
    with pytest.raises(actions.ActionRefused, match="invalid text"):
        actions.plan("L", ProjectTarget(REPO), {"text": text}, OWNER)


def test_StartLane_OptionalInputsIncludePaceAndTrackReservation():
    action = actions.ACTIONS["L"]
    assert action.needs(ProjectTarget(REPO)) == ("text",) and action.optional == ("preset", "level", "track", "op")


def test_StartLane_APresetAndALevel_ReachTheNewLanesConfigAtCreation():
    from sakura.flow import cli as flow_cli
    from sakura.flow import config, flags

    invocation = actions.plan("L", ProjectTarget(REPO), {"text": "x", "preset": "lean", "level": "fast"}, OWNER)
    args = flow_cli._parser().parse_args(invocation.argv[2:])
    overrides = flags.build_overrides(args.preset, args.dial, args.familiarity, args.role, args.settings, args.relax)
    values = config.load(None, overrides=overrides).values
    assert values["knobs"]["lane_pace"] == "fast" and values["knobs"]["final_review"] is True


@pytest.mark.parametrize(("inputs", "sentence"), [
    ({"preset": "nonesuch"}, "invalid preset: not a known preset"),
    ({"level": "warp"}, "invalid level: use one of slowest, slow, normal, fast, fastest"),
])

def test_StartLane_AnUnknownPresetOrLevel_IsRefusedWithAFixedSentenceAndNothingPlanned(inputs, sentence):
    with pytest.raises(actions.ActionRefused) as refused:
        actions.plan("L", ProjectTarget(REPO), {"text": "x", **inputs}, OWNER)
    assert str(refused.value) == sentence


@pytest.mark.parametrize("level", ["slowest", "slow", "normal", "fast", "fastest"])
def test_StartLane_EveryPaceTheSheetOffers_IsAccepted(level):
    invocation = actions.plan("L", ProjectTarget(REPO), {"text": "x", "level": level}, OWNER)
    assert f"--set=knobs.lane_pace={level}" in invocation.argv


def test_StartLane_CeilingRefusal_ReachesTheClientWithTheLaneNames():
    spawns = Spawns(stdout="the master's ceiling is 1 open lane and 1 are open: repo/first. Finish it.\n", returncode=2)
    done, _ = invoke("L", ProjectTarget(REPO), {"text": "more"}, spawns)
    assert "ceiling is 1" in actions.refusal("L", done) and "repo/first" in actions.refusal("L", done)
    spawns = Spawns(stdout="Traceback at /home/x/secret.py\n", returncode=2)
    done, _ = invoke("L", ProjectTarget(REPO), {"text": "more"}, spawns)
    assert actions.refusal("L", done) == actions.START_FAILED_TEXT


def test_ChildEnvironment_MarksTheCockpitsOwnChildSoItMayCarryAOverlayActor(monkeypatch):
    from sakura import actors

    monkeypatch.delenv(actors.TRUSTED_ENV, raising=False)
    assert actions._child_environment()[actors.TRUSTED_ENV] == "1"


def test_FlowCommandLine_OverlayActorFromATerminal_RefusedAndNothingQueued(repo, monkeypatch, capsys):
    from flow_support import make_context
    from sakura import actors
    from sakura.flow import cli, exits

    ctx = make_context(repo)
    monkeypatch.chdir(repo)
    monkeypatch.delenv(actors.TRUSTED_ENV, raising=False)
    assert cli.main(["steer", "--actor=overlay:owner@example.com:nPHONE", "demo", "hello"]) == exits.USAGE
    assert "cannot claim" in capsys.readouterr().out and inbox.pending(ctx.store) == []
    monkeypatch.setenv(actors.TRUSTED_ENV, "1")
    assert cli.main(["steer", "--actor=overlay:owner@example.com:nPHONE", "demo", "hello"]) == exits.OK


@pytest.mark.parametrize("choices", ['{"kind":"feed","label":"News","address":"https://example.test","token":"hidden"}',
                                     '{"kind":"feed","label":"News\\nToday","address":"https://example.test"}'])
def test_connector_add_refuses_unlisted_fields_and_decoded_controls(choices):
    from sakura.cockpit.host_actions import CONNECTORS

    with pytest.raises(actions.ActionRefused, match="invalid_choices"):
        actions.plan("connectoradd", CONNECTORS, {"op": "new-feed", "choices": choices}, Actor("cli", "owner"))


def test_connector_actions_are_local_only():
    assert actions.is_local_only("connectordisconnect", ("host", "connectors"))


def test_connector_action_routes_explicit_options_to_the_engine(tmp_path):
    import io

    from sakura.cockpit.host_actions import CONNECTORS
    from sakura.connectors import cli

    inputs = {"op": "new-feed", "choices": json.dumps({"kind": "feed", "label": "News",
                                                      "address": "https://example.test/feed"})}
    invocation = actions.plan("connectoradd", CONNECTORS, inputs, Actor("cli", "owner"), log_path=tmp_path / "log")
    assert "--choices" not in " ".join(invocation.argv)
    assert "--address=https://example.test/feed" in invocation.argv
    out = io.StringIO()
    assert cli.main(invocation.argv[2:], stdout=out) == 0
    assert json.loads(out.getvalue())["revision"] == 1


def test_connector_mutation_cannot_be_routed_through_an_unrelated_host_target():
    from sakura.cockpit.host_actions import ENGINE

    with pytest.raises(actions.ActionRefused, match="invalid_request"):
        actions.plan("connectordisconnect", ENGINE, {"op": "remove", "name": "cn-news", "revision": "1"},
                     Actor("cli", "owner"))
