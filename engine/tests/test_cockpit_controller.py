import json
import re
from pathlib import Path

import pytest

from sakura.actors import Actor
from sakura.cockpit import actions, api, gate, items, pages, proofs, wait_actions
from sakura.cockpit.controller import Confirm, Controller, Done, Refused, overlay_actor
from sakura.cockpit.items import GroupRow
from sakura.flow import inbox, store, waits
from test_cockpit_actions import Spawns, flow_item, lane_item, log_rows, wait_target
from test_cockpit_snapshot import create_task, make_snapshot, other_repo

SECRET = b"c" * 32
OWNER = gate.Identity("owner@example.com", "nPHONE1", "Owner's phone")
MAC = gate.Identity("owner@example.com", "nMAC", "mac")
ACTOR_OPTION = "--actor=overlay:owner@example.com:nPHONE1"
REPO = Path("/work/repo")
NOW = 7_000_000.0


class Clock:
    def __init__(self):
        self.now = NOW

    def __call__(self):
        return self.now


class Fresh:
    """A snapshot source that counts how often it is read."""

    def __init__(self, snapshot):
        self.snapshot, self.reads = snapshot, 0

    def __call__(self):
        self.reads += 1
        return self.snapshot


@pytest.fixture
def world(tmp_path):
    flow, lane = flow_item(REPO, "t1"), lane_item(REPO, "proc", "auth")
    group = GroupRow("proc", REPO, "proc", 1, "", "")
    snapshot = make_snapshot(items=[flow, lane], groups=[group])
    spawns, clock, fresh = Spawns(stdout="engine said ok\n"), Clock(), Fresh(snapshot)
    cached = Fresh(snapshot)
    log = tmp_path / "cockpit" / "actions.jsonl"
    controller = Controller(SECRET, cached, fresh, run=spawns, log_path=log, clock=clock)
    return type("World", (), dict(flow=flow, lane=lane, group=group, spawns=spawns, clock=clock, fresh=fresh,
                                  cached=cached, log=log, controller=controller))


def form(target, key, **inputs):
    fields = {"key": key, "target": json.dumps(list(target.key))}
    fields.update({f"input.{name}": value for name, value in inputs.items()})
    return fields


def act(world, fields, identity=OWNER):
    return world.controller.act(fields, identity, "csrf-token")


def proof_of(page):
    return re.search(r'name="proof" value="([^"]+)"', page).group(1)


def confirmed(fields, page):
    return {**fields, "proof": proof_of(page)}


def test_index_uses_the_cached_snapshot_and_embeds_the_csrf_token(world):
    page = world.controller.index(OWNER, "csrf-token")
    assert "csrf-token" in page and world.cached.reads == 1 and world.fresh.reads == 0


def test_a_plain_action_runs_in_one_step_and_shows_the_result(world):
    page = act(world, form(world.flow, "D", step="strong"))
    (argv, kwargs), = world.spawns.calls
    assert argv[-2:] == ["t1", "strong"] and ACTOR_OPTION in argv and kwargs["cwd"] == REPO
    assert "exit code 0" in page and "engine said ok" in page


def test_a_refused_action_shows_why_and_runs_nothing(world):
    page = act(world, form(world.flow, "D", step="../strong"))
    assert "refused" in page and "invalid step" in page and world.spawns.calls == []
    assert [(row["outcome"], row["key"]) for row in log_rows(world.log)] == [("refused", "D")]


def test_optional_blank_input_is_dropped_and_a_blank_required_one_is_refused(world):
    fields = form(world.flow, "a", remember="")
    asked = _perform(world, fields, source="cli")
    assert isinstance(_perform(world, {**fields, "proof": asked.token}, source="cli"), Done)
    (argv, _), = world.spawns.calls
    assert "approve" in argv and "--remember" not in argv
    assert "invalid step" in act(world, form(world.flow, "D", step=""))


def test_confirmed_action_asks_first_and_runs_only_with_its_proof(world):
    fields = form(world.flow, "s")
    first = act(world, fields)
    assert world.spawns.calls == [] and "Confirm" in first and proof_of(first)
    assert ACTOR_OPTION in first  # the exact command, actor included
    second = act(world, confirmed(fields, first))
    assert len(world.spawns.calls) == 1 and "exit code 0" in second


def test_valid_two_step_flow_runs_once(world):
    fields = form(world.flow, "a")
    asked = _perform(world, fields, source="cli")
    proven = {**fields, "proof": asked.token}
    assert isinstance(_perform(world, proven, source="cli"), Done)
    replay = _perform(world, proven, source="cli")
    assert isinstance(replay, Refused) and replay.denied
    (argv, kwargs), = world.spawns.calls
    assert argv[1:4] == ["flow", "approve", "--continue"] and kwargs["cwd"] == REPO
    assert [row["outcome"] for row in log_rows(world.log)] == ["done", "refused"]


@pytest.mark.parametrize("extra", [{"confirmed": "1"}, {"confirm": "yes"}, {"cwd": "/etc"}, {"repo": "/etc"},
                                    {"input": "x"}, {"proof ": "x"}])
def test_confirmed_flag_without_proof_denied(world, extra):
    with pytest.raises(gate.Denied):
        act(world, {**form(world.flow, "s"), **extra})
    assert world.spawns.calls == []
    assert log_rows(world.log)[-1]["outcome"] == "refused"


def test_confirmed_action_without_a_proof_never_runs(world):
    act(world, form(world.flow, "s"))
    act(world, {**form(world.flow, "s"), "proof": ""})
    assert world.spawns.calls == []


@pytest.mark.parametrize("change", ["other_key", "other_target", "other_identity", "expired", "garbage"])
def test_proof_mismatch_expired_or_replayed_denied(world, change):
    other = lane_item(REPO, "proc", "auth")
    fields = form(world.flow, "s")
    proof = proof_of(act(world, fields))
    identity, tried = OWNER, {**fields, "proof": proof}
    if change == "other_key":
        tried["key"] = "R"
    elif change == "other_target":
        tried["target"] = json.dumps(list(other.key))
    elif change == "other_identity":
        identity = MAC
    elif change == "expired":
        world.clock.now += 121
    else:
        tried["proof"] = proof[:-1] + ("0" if proof[-1] != "0" else "1")
    with pytest.raises(gate.Denied):
        act(world, tried, identity)
    assert world.spawns.calls == []


def test_target_resolved_server_side(world):
    forged = {"key": "D", "target": json.dumps(["flow", "/etc", "t1"]), "input.step": "strong"}
    page = act(world, forged)
    assert "no such" in page and world.spawns.calls == []
    assert world.fresh.reads == 1 and world.cached.reads == 0  # always a fresh snapshot, never the cached page one
    act(world, form(world.flow, "D", step="strong"))
    (_, kwargs), = world.spawns.calls
    assert kwargs["cwd"] == world.flow.repo and world.fresh.reads == 2


@pytest.mark.parametrize("target", ["", "not json", "{}", "[]", "[1, 2, 3]", '["flow"]', '["a","b","c","d"]',
                                    json.dumps(["flow", "/x", ["y"]])])
def test_malformed_target_is_denied(world, target):
    with pytest.raises(gate.Denied):
        act(world, {"key": "s", "target": target})
    assert world.spawns.calls == []


def test_deeply_nested_target_is_denied_and_logged_not_a_crash(world):
    nested = "[" * 200_000 + "]" * 200_000
    with pytest.raises(gate.Denied):
        act(world, {"key": "s", "target": nested})
    assert world.spawns.calls == []
    assert log_rows(world.log)[-1]["outcome"] == "refused"


def test_missing_key_or_target_is_denied(world):
    for fields in ({}, {"key": "s"}, {"target": json.dumps(list(world.flow.key))}):
        with pytest.raises(gate.Denied):
            act(world, fields)


def test_a_wrong_kind_for_the_target_is_refused_not_run(world):
    assert "does not apply" in act(world, form(world.lane, "R"))
    assert world.spawns.calls == []


def test_hostile_values_in_a_refusal_are_escaped(world):
    page = act(world, {"key": "<script>x</script>", "target": json.dumps(list(world.flow.key))})
    assert "<script>x" not in page and "&lt;script&gt;" in page


def test_actor_is_overlay_identity_in_ledger_and_action_log(tmp_path, repo):
    second = other_repo(tmp_path)
    for target in (repo, second):
        create_task(target, "shared-id", status="running")
    chosen = next(i for i in items.flow_items(second, lambda key: None) if i.key[2] == "shared-id")
    snapshot = make_snapshot(items=[chosen])
    log = tmp_path / "actions.jsonl"
    controller = Controller(SECRET, lambda: snapshot, lambda: snapshot, log_path=log, clock=Clock())
    page = controller.act(form(chosen, "D", step="power"), OWNER, "csrf")
    assert "exit code 0" in page
    pending = {r: [c for _, c in inbox.pending(store.TaskStore(r, "shared-id"))] for r in (repo, second)}
    assert pending[repo] == []
    assert [(c["kind"], c["value"], c["actor"]) for c in pending[second]] == [
        ("dial", "power", "overlay:owner@example.com:nPHONE1")]
    (row,) = log_rows(log)
    assert row["actor"] == {"source": "overlay", "login": "owner@example.com", "node_id": "nPHONE1",
                            "device": "Owner's phone"}


def test_page_forms_post_what_the_controller_accepts(world):
    page = pages.index(make_snapshot(items=[world.flow]), OWNER, "csrf-token")
    assert f'name="target" value="{pages.text(json.dumps(list(world.flow.key)))}"' in page


def test_a_damaged_proof_store_gives_the_client_a_fixed_reason_and_logs_the_detail_to_stderr(world, monkeypatch, capsys):
    def damaged(*args, **kwargs):
        raise proofs.ProofStoreError("used proofs file /home/owner/state/nonces.json is unreadable\x1b]2;x\x07")

    fields = form(world.flow, "s")
    first = act(world, fields)
    monkeypatch.setattr(proofs, "consume", damaged)
    page = act(world, confirmed(fields, first))
    assert "/home/owner" not in page and "cannot check the confirmation" in page and world.spawns.calls == []
    err = capsys.readouterr().err
    assert "/home/owner/state/nonces.json" in err and "\x1b" not in err and "\x07" not in err


def _perform(world, fields, source="overlay"):
    actor = Actor("cli", "owner") if source == "cli" else overlay_actor(OWNER)
    return world.controller.perform(fields, actor, proof_field="proof", proof_source=source)


def _waiting(world, *targets):
    world.fresh.snapshot = make_snapshot(items=[world.flow], waits=[target.wait for target in targets])


def test_Act_ResolvedWait_RefusedNoLongerWaiting(world):
    target = wait_target("approval", "checks")
    _waiting(world, target)
    fields = form(target, "a")
    asked = _perform(world, fields, source="cli")
    assert isinstance(asked, Confirm)
    assert isinstance(_perform(world, {**fields, "proof": asked.token}, source="cli"), Done)

    _waiting(world)  # the engine moved on: the gate is answered
    again = _perform(world, fields, source="cli")

    assert isinstance(again, Refused) and not again.denied and "no longer waiting" in again.reason
    assert len(world.spawns.calls) == 1


def test_Act_ConfirmedAfterTheWaitResolved_RefusedAndTheProofIsKept(world):
    target = wait_target("approval", "checks")
    _waiting(world, target)
    fields = form(target, "a")
    asked = _perform(world, fields, source="cli")

    _waiting(world)
    late = _perform(world, {**fields, "proof": asked.token}, source="cli")

    assert isinstance(late, Refused) and "no longer waiting" in late.reason and world.spawns.calls == []


def test_Act_MarkOnCheckRow_Refused(world):
    row_wait = wait_target("owner_row", "R1")
    check_row = wait_target("owner_row", "R2")  # R2 is proven by a check: the engine lists no wait for it
    _waiting(world, row_wait)

    outcome = _perform(world, form(check_row, "k", verdict="ok"), source="cli")

    assert isinstance(outcome, Refused) and "no longer waiting" in outcome.reason and world.spawns.calls == []
    assert isinstance(_perform(world, form(row_wait, "k", verdict="ok"), source="cli"), Done)
    assert world.spawns.calls[0][0][-3:] == ["t1", "R1", "ok"]


# -- typed answers get a page of their own (outside lanes on the phone) -------------------------------------


def outside_world(world):
    item = items.Item("outside", ("outside", "measure-help"), Path("/work/work"), None, "", "", "", "waiting", "",
                      {}, [], None, "", title="Add --help", author="outside:claude")
    identity = json.dumps(["wait", "outside", "measure-help", "question", "why@0123456789ab"], separators=(",", ":"))
    question = waits.Wait(identity, "question", "/work/work", "work", "measure-help", "Why?", "Why?", None,
                          "outside", "outside:claude", ("yes", "no"))
    world.fresh.snapshot = make_snapshot(items=[world.flow, world.lane, item], waits=[question])
    return item, wait_actions.target_of(question)


def test_Act_HtmlWriteWithoutText_AnswersComposePageWithoutRefresh(world):
    item, _ = outside_world(world)
    page = act(world, form(item, "w"))
    assert "<textarea" in page and 'name="input.text"' in page and 'http-equiv="refresh"' not in page
    assert 'name="key" value="w"' in page and "Add --help" in page and world.spawns.calls == []
    flow = act(world, form(world.flow, "w"))
    assert "<textarea" in flow and 'name="key" value="w"' in flow and world.spawns.calls == []
    assert "<textarea" in act(world, form(world.lane, "w"))


def test_Act_HtmlReplyWithoutChoiceOrText_AnswersComposePage(world):
    _, question = outside_world(world)
    page = act(world, form(question, "r"))
    assert "<textarea" in page and 'name="key" value="r"' in page and "Why?" in page
    assert 'http-equiv="refresh"' not in page and world.spawns.calls == []


def test_Act_HtmlReplyWithChoice_NoComposePage(world):
    _, question = outside_world(world)
    page = act(world, form(question, "r", choice="maybe"))
    assert "not one of the choices" in page and "<textarea" not in page


def test_Act_HtmlWriteWithTextOnTheComposePage_Runs(world):
    outside_world(world)
    page = act(world, form(world.flow, "w", text="prefer short names"))
    assert "exit code 0" in page and len(world.spawns.calls) == 1


def test_Act_ComposeForAnUnknownTargetOrWrongKind_NotAComposePage(world):
    outside_world(world)
    assert "<textarea" not in act(world, {"key": "w", "target": json.dumps(["flow", "/etc", "t1"])})
    assert "<textarea" not in act(world, form(world.group, "w"))
    assert "<textarea" not in act(world, form(world.flow, "s"))


def test_Act_JsonWriteWithoutText_StillRefused(world):
    outcome = _perform(world, form(world.flow, "w"))
    assert isinstance(outcome, Refused) and "missing text" in outcome.reason
    _, question = outside_world(world)
    reply = _perform(world, form(question, "r"))
    assert isinstance(reply, Refused) and "one of them" in reply.reason


def test_inbox_stop_confirmation_cannot_move_to_a_new_lane_revision(world):
    from dataclasses import replace

    target = wait_target(waits.BLOCKED, "failed-check")
    lane = replace(world.flow, status="blocked", resume_revision="state-a")
    world.fresh.snapshot = make_snapshot(items=[lane], waits=[target.wait])
    fields = form(target, "s")
    asked = _perform(world, fields)
    assert isinstance(asked, Confirm)

    world.fresh.snapshot = make_snapshot(items=[replace(lane, resume_revision="state-b")], waits=[target.wait])
    late = _perform(world, {**fields, "proof": asked.token})
    assert isinstance(late, Refused) and late.denied and not world.spawns.calls



def test_browser_resume_binds_the_normalized_multiline_note(world):
    from dataclasses import replace

    lane = replace(world.flow, status="blocked", agent_state="blocked", resume_revision="state-a",
                   next_command="sakura flow resume t1")
    world.fresh.snapshot = make_snapshot(items=[lane])
    fields = form(lane, "R", note="First line\r\nSecond line")
    asked = _perform(world, fields)
    assert isinstance(asked, Confirm) and asked.inputs["note"] == "First line\nSecond line"

    result = _perform(world, {**form(lane, "R", note="First line\nSecond line"), "proof": asked.token})
    assert isinstance(result, Done)
    [(argv, _)] = world.spawns.calls
    assert "--note=First line\nSecond line" in argv



def test_phone_faces_apply_host_policy_before_resolving_targets(world):
    fields = form(world.flow, "a")
    phone = {**fields, "target": world.controller.remote_ids.register(world.flow.key)}
    status, reply = api.Api(world.controller).action(phone, OWNER)
    assert status == 200 and reply["result"] == "refused"
    assert "only available on the machine" in world.controller.phone_act(phone, OWNER, "csrf-token")
    assert world.fresh.reads == 0 and world.spawns.calls == []


def test_remote_snapshot_and_confirmation_keep_host_paths_out(world):
    dashboard = api.Api(world.controller)
    public = dashboard.snapshot()
    item = public["items"][0]
    assert str(REPO) not in json.dumps(public)
    assert item["id"] != api.target_id(world.flow)

    status, confirmation = dashboard.action({"key": "s", "target": item["id"]}, OWNER)

    assert status == 200 and confirmation["result"] == "confirm"
    assert str(REPO) not in json.dumps(confirmation)
    assert "command" not in confirmation
    html = world.controller.index(OWNER, "csrf-token")
    assert str(REPO) not in html and item["id"] in html
    confirmed = {"key": "s", "target": item["id"], "proof": confirmation["token"]}
    page = world.controller.phone_act(confirmed, OWNER, "csrf-token")
    assert "exit code 0" in page and "engine said ok" not in page


def test_cockpit_audit_records_action_without_raw_arguments(world):
    note = "discard-this-private-fixture"
    assert isinstance(_perform(world, form(world.flow, "w", text=note)), Done)

    assert note not in world.log.read_text()
