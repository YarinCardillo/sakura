"""Write actions on accounts: the notes app renames, caps, disables and adds plan periods through the cockpit."""

import hashlib
import json
import re
from pathlib import Path

import pytest

from sakura import cli as sakura_cli
from sakura.accounts import registry
from sakura.accounts.providers import ADAPTERS
from sakura.cockpit import account_actions, actions, api, cli, gate, remote
from sakura.actors import Actor
from sakura.cockpit.accounts import AccountRow
from sakura.cockpit.controller import FIELD_CONFIRM, PROOF_SOURCE_CLI, REMOTE_REFUSAL, Controller, Refused
from sakura.flow import exits
from test_cockpit_actions import Spawns, log_rows
from test_cockpit_snapshot import NoTmux, make_snapshot

SECRET = b"c" * 32
PHONE = gate.Identity("owner@example.com", "nPHONE1", "Owner's phone")
HOST = Actor("cli", "owner@example.com", "nPHONE1", "Owner's phone")
ACTOR_OPTION = "--actor=cli:owner@example.com:nPHONE1"
TARGET = '["account","claude","work"]'
USAGE_LINE = "usage: sakura accounts [-h] {detect,list,add,rename} ...\n"
LEGACY_STDERR = USAGE_LINE + ("sakura: error: /home/o/.config/sakura/usage.toml still holds the accounts; "
                              "run sakura accounts migrate first\n")


def account_row(provider="claude", account_id="work", **over) -> AccountRow:
    fields = dict(provider=provider, id=account_id, key=account_id, label="Work", state="ok", enabled=True,
                  week_pct=None, week_resets_at=None, five_hour_pct=None, reserve_week_pct=None, open_alerts=0)
    return AccountRow(**{**fields, **over})


@pytest.fixture
def world(tmp_path):
    spawns = Spawns(stdout="")
    snapshot = make_snapshot(accounts=[account_row(), account_row("codex", "main", key="codex:main")])
    log = tmp_path / "cockpit" / "actions.jsonl"
    controller = Controller(SECRET, lambda: snapshot, lambda: snapshot, run=spawns, log_path=log)
    return type("World", (), dict(spawns=spawns, log=log, snapshot=snapshot, controller=controller))


def host_action(controller, fields) -> tuple[int, dict]:
    """What `sakura cockpit act` does with a request: the exit code and the local answer document."""
    outcome = controller.perform(fields, HOST, proof_field=FIELD_CONFIRM, proof_source=PROOF_SOURCE_CLI)
    return (exits.USAGE if isinstance(outcome, Refused) else exits.OK), api.action_document(outcome)


def post(world, key, target=TARGET, **inputs):
    fields = {"key": key, "target": target, **{f"input.{name}": value for name, value in inputs.items()}}
    return host_action(world.controller, fields)


def phone_post(controller, key, target=TARGET) -> tuple[int, dict]:
    """A request over the phone transport, which sends the opaque reference of a target, never its raw key."""
    reference = controller.remote_ids.register(tuple(json.loads(target)))
    return api.Api(controller).action({"key": key, "target": reference}, PHONE)


def test_snapshot_account_rows_carry_target_and_actions():
    document = api.snapshot_document(make_snapshot(accounts=[account_row("codex", "main", key="codex:main")]))

    [row] = document["accounts"]

    assert row["target"] == '["account","codex","main"]'
    assert [(a["key"], a["label"], a["needs"], a["confirm"]) for a in row["actions"]] == [
        ("l", "Rename", ["label"], False), ("c", "Set reserve", ["percent"], False),
        ("n", "Disable", [], True), ("e", "Enable", [], False), ("p", "Add plan period", ["plan", "since"], False),
        ("X", "Remove", [], True)]


def test_action_keys_are_unique_and_leave_the_reserved_ones_free():
    assert len(actions.ACTIONS) == len({action.key for action in actions.ACTIONS.values()})
    assert not {"x", "z"} & set(actions.ACTIONS)


def test_rename_action_runs_the_cli_and_logs_the_actor(world):
    status, document = post(world, "l", label="Work account")

    assert (status, document["result"], document["exit_code"]) == (exits.OK, "done", 0)
    [(argv, kwargs)] = world.spawns.calls
    assert argv[1:] == ["accounts", "rename", ACTOR_OPTION, "--", "claude", "work", "Work account"]
    assert kwargs["cwd"] == Path("/") and str(Path.home()) not in " ".join(argv)
    [row] = log_rows(world.log)
    assert (row["key"], row["target"], row["outcome"], row["actor"]["login"]) == (
        "l", {"kind": "account", "id": hashlib.sha256(TARGET.encode()).hexdigest()}, "done", "owner@example.com")


def test_each_action_builds_its_cli_command(world):
    for key, inputs, tail in (("c", {"percent": "35.5"}, ["set-reserve", ACTOR_OPTION, "--", "claude", "work", "35.5"]),
                              ("c", {"percent": "none"}, ["set-reserve", ACTOR_OPTION, "--", "claude", "work", "none"]),
                              ("e", {}, ["enable", ACTOR_OPTION, "--", "claude", "work"]),
                              ("p", {"plan": "max20x", "since": "2026-10-29"},
                               ["plan", ACTOR_OPTION, "--", "claude", "work", "max20x", "2026-10-29"])):
        world.spawns.calls.clear()
        assert post(world, key, **inputs)[1]["result"] == "done"
        assert world.spawns.calls[0][0][2:] == tail


def test_a_value_starting_with_a_dash_stays_after_the_separator(world):
    post(world, "l", label="--help")

    [(argv, _)] = world.spawns.calls
    assert argv[argv.index("--"):] == ["--", "claude", "work", "--help"]


def test_disable_needs_the_two_step_confirm(world):
    status, asked = post(world, "n")

    assert (status, asked["result"]) == (exits.OK, "confirm") and world.spawns.calls == []
    assert "accounts disable" in asked["summary"]
    status, done = host_action(world.controller, {"key": "n", "target": TARGET, "confirm": asked["token"]})
    assert (status, done["result"]) == (exits.OK, "done")
    [(argv, _)] = world.spawns.calls
    assert argv[1:] == ["accounts", "disable", ACTOR_OPTION, "--", "claude", "work"]
    replay = host_action(world.controller, {"key": "n", "target": TARGET, "confirm": asked["token"]})
    assert replay[0] == exits.USAGE and len(world.spawns.calls) == 1


@pytest.mark.parametrize("key, inputs", [
    ("l", {"label": ""}), ("l", {"label": "x" * 61}), ("l", {"label": "bad\x07bell"}), ("l", {"label": "two\nlines"}),
    ("l", {"label": "   "}), ("l", {"label": "zero\u200bwidth"}),
    ("c", {"percent": "101"}), ("c", {"percent": "-1"}), ("c", {"percent": "abc"}), ("c", {"percent": "1e2"}),
    ("c", {"percent": "NONE"}), ("c", {"percent": ""}),
    ("p", {"plan": "Max 20", "since": "2026-10-29"}), ("p", {"plan": "-max", "since": "2026-10-29"}),
    ("p", {"plan": "a" * 33, "since": "2026-10-29"}), ("p", {"plan": "plus", "since": "2026-10-29"}),
    ("p", {"plan": "max5", "since": "2026-10-29"}), ("p", {"plan": "a" * 32, "since": "2026-10-29"}), ("p", {"plan": "max", "since": "yesterday"}),
    ("p", {"plan": "max", "since": "2026-13-01"}), ("p", {"plan": "max", "since": "2026-10-29T25:00:00"}),
    ("p", {"plan": "max"}), ("l", {}), ("l", {"label": "ok", "extra": "x"}), ("n", {"label": "x"})])
def test_bad_inputs_are_refused_before_spawning(world, key, inputs):
    status, document = post(world, key, **inputs)

    assert (status, document["result"]) == (exits.USAGE, "refused") and world.spawns.calls == []
    assert [row["outcome"] for row in log_rows(world.log)] == ["refused"]


@pytest.mark.parametrize("key, inputs", [
    ("l", {"label": "x" * 60}), ("l", {"label": "Café 日本"}), ("c", {"percent": "0"}), ("c", {"percent": "100"}),
    ("p", {"plan": "pro", "since": "2026-10-29T08:30:00+02:00"}),
    ("p", {"plan": "max5x", "since": "2026-10-29T08:30:00Z"}), ("p", {"plan": "max20x", "since": "2026-10-29 08:30"})])
def test_edge_values_that_fit_the_rules_are_accepted(world, key, inputs):
    assert post(world, key, **inputs)[1]["result"] == "done"


def test_an_account_target_that_is_not_in_the_snapshot_is_refused(world):
    status, document = post(world, "e", target='["account","claude","ghost"]')

    assert (status, document["result"]) == (exits.USAGE, "refused") and world.spawns.calls == []


def test_account_actions_do_not_apply_to_other_targets_and_vice_versa(world):
    assert post(world, "e", target='["group","proc"]')[1]["result"] == "refused"
    assert world.spawns.calls == []


def test_legacy_host_refusal_comes_back_as_refused(tmp_path):
    spawns = Spawns(stderr=LEGACY_STDERR, returncode=2)
    snapshot = make_snapshot(accounts=[account_row()])
    controller = Controller(SECRET, lambda: snapshot, lambda: snapshot, run=spawns, log_path=tmp_path / "log.jsonl")

    status, document = host_action(controller, {"key": "e", "target": TARGET})

    assert (status, document["result"]) == (exits.USAGE, "refused")
    assert document["reason"] == account_actions.LEGACY_HOST_TEXT
    assert "run sakura accounts migrate first" in document["reason"]
    assert [row["outcome"] for row in log_rows(tmp_path / "log.jsonl")] == ["failed"]


def refusal_of(stderr: str, tmp_path) -> str:
    spawns = Spawns(stderr=stderr, returncode=2)
    snapshot = make_snapshot(accounts=[account_row()])
    controller = Controller(SECRET, lambda: snapshot, lambda: snapshot, run=spawns, log_path=tmp_path / "log.jsonl")
    document = host_action(controller, {"key": "e", "target": TARGET})[1]
    assert document["result"] == "refused"
    return document["reason"]


def test_an_unknown_account_refusal_keeps_the_cli_message_on_one_line(tmp_path):
    reason = refusal_of(USAGE_LINE + "sakura: error: no account claude/ghost\n", tmp_path)

    assert reason == "no account claude/ghost"


@pytest.mark.parametrize("message", [
    "/home/o/.config/sakura/accounts.toml: cannot read accounts registry: [Errno 13] Permission denied",
    "accounts.toml: accounts has unknown keys ['x']",
    "cannot read accounts registry: [Errno 13] Permission denied: '/home/o/.config/sakura/accounts.toml'",
    "~/.config/sakura/accounts.toml: not valid UTF-8", "../accounts.tmp: no", "C:\\Users\\o\\accounts.toml: no"])
def test_refusal_text_has_no_path(tmp_path, message):
    reason = refusal_of(USAGE_LINE + f"sakura: error: {message}\n", tmp_path)

    assert reason == account_actions.UNREADABLE_TEXT
    assert not re.search(r"[/\\~]|\.toml", reason)


def test_a_refusal_without_a_message_line_gets_the_fixed_text(tmp_path):
    assert refusal_of("Traceback (most recent call last):\n  File \"/x/y.py\"\n", tmp_path) == account_actions.UNREADABLE_TEXT


def test_a_long_or_multiline_message_is_cut_to_one_line(tmp_path):
    reason = refusal_of(USAGE_LINE + "sakura: error: " + "word " * 100 + "\nsecond line\n", tmp_path)

    assert "\n" not in reason and len(reason) <= actions.MAX_LINE


def test_a_failed_command_other_than_a_refusal_stays_done_with_its_exit_code(tmp_path):
    spawns = Spawns(stderr="boom\n", returncode=exits.FAILED)
    snapshot = make_snapshot(accounts=[account_row()])
    controller = Controller(SECRET, lambda: snapshot, lambda: snapshot, run=spawns, log_path=tmp_path / "log.jsonl")

    document = host_action(controller, {"key": "e", "target": TARGET})[1]

    assert (document["result"], document["exit_code"]) == ("done", exits.FAILED)


def test_the_argv_the_cockpit_builds_is_accepted_by_the_real_cli(tmp_path, monkeypatch):
    home = tmp_path / "isolated-home"
    (home / "dir").mkdir()
    monkeypatch.setenv("HOME", str(home))
    assert sakura_cli.main(["accounts", "add", "--provider", "claude", "--id", "work",
                              "--config-dir", str(home / "dir")]) == 0
    target = account_actions.AccountTarget("claude", "work")
    steps = (("l", {"label": "-Dash"}), ("c", {"percent": "30"}), ("n", {}), ("e", {}),
             ("p", {"plan": "max5x", "since": "2026-10-29"}))

    for key, inputs in steps:
        invocation = actions.plan(key, target, inputs, gate_actor())
        assert sakura_cli.main(invocation.argv[1:]) == 0, key

    [stored] = registry.load().accounts
    assert (stored.label, stored.reserve_week_pct, stored.enabled, [p.plan for p in stored.plans]) == (
        "-Dash", 30.0, True, ["max5x"])


def gate_actor():
    from sakura.cockpit.controller import overlay_actor
    return overlay_actor(PHONE)


DEFAULT_TARGET = '["account","claude","default"]'


def test_cockpit_act_accepts_account_inputs_as_flags(tmp_path, capsys):
    (tmp_path / "isolated-home" / ".claude" / "projects").mkdir(parents=True)
    spawns = Spawns()

    code = cli.main(["act", "p", DEFAULT_TARGET, "--plan", "max20x", "--since", "2026-10-29", "--actor",
                     "owner@example.com", "--json"], client=NoTmux(), run=spawns)
    answer = json.loads(capsys.readouterr().out)

    assert code == exits.OK and answer["result"] == "done"
    [(argv, _)] = spawns.calls
    assert argv[argv.index("--"):] == ["--", "claude", "default", "max20x", "2026-10-29"]


def test_cockpit_act_refusal_exits_with_usage(tmp_path, capsys):
    (tmp_path / "isolated-home" / ".claude" / "projects").mkdir(parents=True)

    code = cli.main(["act", "c", DEFAULT_TARGET, "--percent", "200", "--json"], client=NoTmux(), run=Spawns())

    assert code == exits.USAGE and json.loads(capsys.readouterr().out)["result"] == "refused"


# -- fix round 1: no home path in confirmations, scrubbed failures, plan choices -----------------------

def test_a_plan_outside_the_providers_choices_is_refused_before_spawning(world):
    claude = post(world, "p", plan="plus", since="2026-10-29")[1]
    codex = post(world, "p", target='["account","codex","main"]', plan="max5x", since="2026-10-29")[1]

    assert claude["result"] == codex["result"] == "refused" and world.spawns.calls == []
    assert "pro, max5x, max20x" in claude["reason"] and "plus, prolite, pro" in codex["reason"]
    assert post(world, "p", target='["account","codex","main"]', plan="prolite", since="2026-10-29")[1]["result"] == "done"


def test_the_real_cli_refuses_a_plan_outside_the_choices_with_the_same_constant(tmp_path, monkeypatch, capsys):
    home = tmp_path / "isolated-home"
    (home / "dir").mkdir()
    monkeypatch.setenv("HOME", str(home))
    sakura_cli.main(["accounts", "add", "--provider", "claude", "--id", "work", "--config-dir", str(home / "dir")])

    with pytest.raises(SystemExit) as stopped:
        sakura_cli.main(["accounts", "plan", "claude", "work", "plus", "2026-10-29"])

    assert stopped.value.code == 2 and ", ".join(ADAPTERS["claude"].plan_choices) in capsys.readouterr().err
    assert registry.load().accounts[0].plans == ()


def test_the_disable_confirmation_names_no_home_directory_or_install_path(tmp_path):
    spawns = Spawns()
    snapshot = make_snapshot(accounts=[account_row()])
    controller = Controller(SECRET, lambda: snapshot, lambda: snapshot, run=spawns, log_path=tmp_path / "log.jsonl")
    shown = host_action(controller, {"key": "n", "target": TARGET})[1]["summary"]

    assert str(Path.home()) not in shown and str(actions.PROGRAM) not in shown and "run in" not in shown
    assert "sakura accounts disable" in shown and "claude work" in shown


TRACEBACK = ('Traceback (most recent call last):\n  File "/home/o/sakura/lib/sakura/accounts/write.py", line 140, '
             'in locked\nPermissionError: [Errno 13] Permission denied: \'/home/o/.config/sakura/accounts.toml.lock\'\n')


def result_of(tmp_path, spawns):
    snapshot = make_snapshot(accounts=[account_row()])
    controller = Controller(SECRET, lambda: snapshot, lambda: snapshot, run=spawns, log_path=tmp_path / "log.jsonl")
    return host_action(controller, {"key": "e", "target": TARGET})[1]


def test_a_crashed_command_gives_a_fixed_line_and_no_path(tmp_path):
    document = result_of(tmp_path, Spawns(stderr=TRACEBACK, returncode=1))

    assert (document["result"], document["exit_code"]) == ("done", 1)
    assert document["output"] == account_actions.UNREADABLE_TEXT
    assert "/" not in document["output"] and "Traceback" not in document["output"]


def test_a_command_that_cannot_start_gives_no_path(tmp_path):
    def cannot_start(argv, **kwargs):
        raise OSError(2, "No such file", "/home/o/sakura/bin/sakura")

    document = result_of(tmp_path, cannot_start)

    assert document["exit_code"] == exits.FAILED and document["output"] == account_actions.UNREADABLE_TEXT


def test_the_output_of_a_successful_account_command_is_not_passed_on(tmp_path):
    document = result_of(tmp_path, Spawns(stdout="wrote /home/o/.config/sakura/accounts.toml\n"))

    assert (document["result"], document["exit_code"], document["output"]) == ("done", 0, "")


@pytest.mark.parametrize("key", ["l", "c", "n", "e", "p", "X"])
def test_the_phone_transport_refuses_every_account_action_before_anything_runs(world, key):
    status, document = phone_post(world.controller, key)

    assert (status, document["result"]) == (200, "refused")
    assert document["reason"] == REMOTE_REFUSAL and world.spawns.calls == []
    page = world.controller.act({"key": key, "target": TARGET}, PHONE, "csrf-token")
    assert REMOTE_REFUSAL in page and world.spawns.calls == []


def test_the_served_page_does_not_offer_account_actions_to_the_phone(world):
    document = api.snapshot_document(world.snapshot)

    assert [row["actions"] for row in document["accounts"]] != [[], []]
    assert not any(action["key"] in {"l", "c", "n", "e", "p", "X"} for action in
                   remote.document(document, world.controller.remote_ids)["accounts"][0]["actions"])


def test_other_actions_still_pass_their_output_through(tmp_path):
    from test_cockpit_actions import flow_item
    item = flow_item("/work/repo")
    snapshot = make_snapshot(items=[item])
    controller = Controller(SECRET, lambda: snapshot, lambda: snapshot, run=Spawns(stdout="stopped /work/repo\n"),
                            log_path=tmp_path / "log.jsonl")

    document = host_action(controller, {"key": "D", "target": api.target_id(item), "input.step": "strong"})[1]

    assert document["output"] == "stopped /work/repo\n"


def test_account_rows_carry_the_reserve_the_stop_point_the_plans_and_the_choices():
    row = account_row(reserve_week_pct=25.0, reserve_week_pct_own=None, plans=(("max5x", "2026-10-20T08:30:00Z"),
                                                                              ("pro", "2026-01-01")))
    [encoded] = api.snapshot_document(make_snapshot(accounts=[row]))["accounts"]

    assert (encoded["reserve_week_pct"], encoded["reserve_week_pct_own"], encoded["stop_at_pct"]) == (25.0, None, 75.0)
    assert encoded["plans"] == [{"plan": "max5x", "since": "2026-10-20T08:30:00Z", "label": "Claude Max 5x"},
                                {"plan": "pro", "since": "2026-01-01", "label": "Claude Pro"}]
    assert encoded["plan_choices"] == ["pro", "max5x", "max20x"]


def test_stop_point_is_null_without_any_reserve_and_exact_for_an_own_one():
    [bare] = api.snapshot_document(make_snapshot(accounts=[account_row()]))["accounts"]
    [own] = api.snapshot_document(make_snapshot(accounts=[account_row(reserve_week_pct=33.3, reserve_week_pct_own=33.3)]))["accounts"]

    assert bare["stop_at_pct"] is None and own["stop_at_pct"] == 66.7 and own["reserve_week_pct_own"] == 33.3


def test_rows_of_a_host_without_accounts_toml_carry_no_actions():
    [row] = api.snapshot_document(make_snapshot(accounts=[account_row(is_writable=False)]))["accounts"]

    assert row["actions"] == [] and row["target"] == TARGET


def test_set_reserve_and_the_old_set_ceiling_write_the_same_value_and_only_the_new_one_is_listed(tmp_path, monkeypatch, capsys):
    home = tmp_path / "isolated-home"
    for name in ("one", "two"):
        (home / name).mkdir(parents=True)
        sakura_cli.main(["accounts", "add", "--provider", "claude", "--id", name, "--config-dir", str(home / name)])
    monkeypatch.setenv("HOME", str(home))

    assert sakura_cli.main(["accounts", "set-reserve", "claude", "one", "35"]) == 0
    assert sakura_cli.main(["accounts", "set-ceiling", "claude", "two", "35"]) == 0
    assert sakura_cli.main(["accounts", "set-reserve", "claude", "two", "none"]) == 0
    assert sakura_cli.main(["accounts", "set-ceiling", "claude", "one", "none"]) == 0

    assert [a.reserve_week_pct for a in registry.load().accounts] == [None, None]
    assert sakura_cli.main(["accounts", "set-ceiling", "claude", "one", "35"]) == 0
    assert sakura_cli.main(["accounts", "set-reserve", "claude", "two", "35"]) == 0
    assert [a.reserve_week_pct for a in registry.load().accounts] == [35.0, 35.0]
    capsys.readouterr()
    with pytest.raises(SystemExit):
        sakura_cli.main(["accounts", "--help"])
    listing = capsys.readouterr().out
    assert "set-reserve" in listing and "set-ceiling" not in listing
