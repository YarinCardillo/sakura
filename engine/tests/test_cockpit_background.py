"""What the app sees of the engine's background work: the keeper, and the controls that install it (E15)."""

import json

import pytest

from sakura import actors

from sakura import keeper
from sakura.cockpit import actions, api, host_actions
from sakura.cockpit.controller import REMOTE_REFUSAL, Controller, Refused, overlay_actor
from sakura.cockpit.gate import Identity
from datetime import datetime, timezone
from test_cockpit_actions import OWNER, Spawns, invoke
from test_cockpit_snapshot import make_snapshot

NOW = datetime(2026, 11, 18, 9, 0, tzinfo=timezone.utc)
SERVICES = host_actions.SERVICES


def test_Services_KeeperOn_RunsKeeperInstall():
    result, spawns = invoke("B", SERVICES, {"name": "keeper", "on": "yes"})
    argv = spawns.calls[0][0]
    assert result.exit_code == 0 and argv[1:3] == ["keeper", "install"] and argv[-1] == "--"


def test_Services_KeeperOff_RunsKeeperUninstall():
    _, spawns = invoke("B", SERVICES, {"name": "keeper", "on": "no"})
    assert spawns.calls[0][0][1:3] == ["keeper", "uninstall"]


def test_Services_PhonePage_RunsThePhonePageSetting():
    _, spawns = invoke("B", SERVICES, {"name": "phone-page", "on": "yes"})
    argv = spawns.calls[0][0]
    assert argv[1:3] == ["keeper", "phone-page"] and argv[-2:] == ["--", "on"]


@pytest.mark.parametrize("inputs", [{"name": "sh -c x", "on": "yes"}, {"name": "keeper", "on": "maybe"}, {"name": "keeper"},
                                    {"name": "digest", "on": "yes"}])
def test_Services_BadInputs_AreRefusedBeforeAnythingRuns(inputs):
    spawns = Spawns()
    with pytest.raises(actions.ActionRefused):
        actions.plan("B", SERVICES, inputs, OWNER)
    assert spawns.calls == []


def test_Services_FromThePhone_IsRefusedAsLocalOnly(tmp_path):
    snapshot = make_snapshot()
    controller = Controller(b"s" * 32, lambda: snapshot, lambda: snapshot, run=Spawns(), log_path=tmp_path / "log.jsonl")
    fields = {"key": "B", "target": api.target_id(SERVICES), "input.name": "keeper", "input.on": "yes"}
    outcome = controller.perform(fields, overlay_actor(Identity("owner@example.com", "nPHONE", "phone")),
                                 proof_field="confirm", proof_source="overlay")
    assert isinstance(outcome, Refused) and outcome.reason == REMOTE_REFUSAL


def test_Snapshot_EngineKeeper_ReportsWhetherItIsOnAndWhatItDid():
    assert api.snapshot_document(make_snapshot())["engine"]["keeper"]["on"] is False
    keeper.run_once(datetime.now(timezone.utc), keeper.Ports(repos=lambda: [], is_alive=lambda e: False,
                                                              resume=lambda *_: None, lanes=lambda: 3))
    found = api.snapshot_document(make_snapshot())["engine"]["keeper"]
    assert found["on"] is True and found["last_restart"] is None and found["phone_page"] is True
    assert found["target"] == '["host","services"]' and [a["key"] for a in found["actions"]] == ["B"]


def test_Snapshot_EngineKeeper_UnreadableState_ShowsAFixedSentenceWithNoPath(capsys):
    keeper.state_path().write_text("not json")
    document = api.snapshot_document(make_snapshot())
    found = document["engine"]["keeper"]
    assert found["on"] is False and found["problem"] == "the keeper's state cannot be read"
    assert str(keeper.state_path().parent) not in json.dumps(document)
    assert str(keeper.state_path()) in capsys.readouterr().err


# -- connectors and the digest's settings (E16)

CONNECTORS, DIGEST_SETTINGS = host_actions.CONNECTORS, host_actions.DIGEST_SETTINGS
PHONE = Identity("owner@example.com", "nPHONE", "phone")


def argv_of(key, target, inputs):
    return actions.plan(key, target, inputs, OWNER).argv


def test_Connect_BuildsTheConnectorsCommandWithTheKindAfterTheSeparator():
    argv = argv_of("O", CONNECTORS, {"kind": "feed", "label": "News", "address": "https://example.com/f.xml"})
    assert argv[1:3] == ["connectors", "connect"] and "--label=News" in argv and "--target=https://example.com/f.xml" in argv
    assert argv[-2:] == ["--", "feed"]


def test_Connect_UnknownKind_IsRefused():
    with pytest.raises(actions.ActionRefused, match="not a kind"):
        actions.plan("O", CONNECTORS, {"kind": "sh -c x"}, OWNER)


def test_Disconnect_NamesTheConnector():
    argv = argv_of("o", CONNECTORS, {"name": "github"})
    assert argv[1:3] == ["connectors", "disconnect"] and argv[-2:] == ["--", "github"]


def test_DigestSettings_GivenFieldsBecomeOptions():
    argv = argv_of("I", DIGEST_SETTINGS, {"at": "08:15", "lookback_hours": "48"})
    assert argv[1:3] == ["digest", "settings"] and "--at=08:15" in argv and "--lookback_hours=48" in argv


def test_DigestSettings_NothingGiven_IsRefused():
    with pytest.raises(actions.ActionRefused, match="at least one"):
        actions.plan("I", DIGEST_SETTINGS, {}, OWNER)


def test_DigestSource_OnOff_BuildsTheSourceCommand():
    argv = argv_of("J", DIGEST_SETTINGS, {"name": "work", "on": "no"})
    assert argv[1:3] == ["digest", "source"] and argv[-3:] == ["--", "work", "off"]


@pytest.mark.parametrize(("key", "target", "inputs"), [
    ("O", CONNECTORS, {"kind": "github"}), ("o", CONNECTORS, {"name": "github"}),
    ("I", DIGEST_SETTINGS, {"at": "08:00"}), ("J", DIGEST_SETTINGS, {"name": "work", "on": "yes"})])
def test_ConnectorAndDigestControls_FromThePhone_AreLocalOnly(tmp_path, key, target, inputs):
    snapshot = make_snapshot()
    controller = Controller(b"s" * 32, lambda: snapshot, lambda: snapshot, run=Spawns(), log_path=tmp_path / "log.jsonl")
    fields = {"key": key, "target": api.target_id(target), **{f"input.{name}": value for name, value in inputs.items()}}
    outcome = controller.perform(fields, overlay_actor(PHONE), proof_field="confirm", proof_source="overlay")
    assert isinstance(outcome, Refused) and outcome.reason == REMOTE_REFUSAL


def test_CockpitActCli_TakesTheNewInputs():
    from sakura.cockpit import cli

    args = cli.act_parser().parse_args(["O", "[]", "--kind=feed", "--address=https://x.example/f", "--on=yes", "--at=08:00",
                                        "--lookback_hours=48", "--language=it", "--account=a", "--json"])
    fields = cli._act_fields(args)
    assert fields["input.kind"] == "feed" and fields["input.address"] == "https://x.example/f"
    assert fields["input.lookback_hours"] == "48" and fields["input.on"] == "yes"


def test_Snapshot_Connectors_AreTheLastCheckWithNoSecret():
    from sakura.connectors import kinds, registry

    secret = "ntn_secret_value"
    registry.connect("notion", "", "", kinds.Ports(keychain=lambda service: secret), NOW)
    document = api.snapshot_document(make_snapshot())
    (row,) = document["connectors"]
    assert row["id"] == "notion" and row["connected"] is True and secret not in json.dumps(document)


def test_Snapshot_Digest_ListsSourcesWithLastGoodRunAndTheSwitches():
    from sakura.digest import config, state

    config.save(config.parse({"at": "06:45", "source": [{"id": "work", "run": ["/opt/work.sh"], "enabled": False},
                                                        {"id": "code", "connector": "github"}]}))
    state.save(state.State(sources={"code": state.SourceState("c", "2026-11-17T06:00:00+00:00")}))
    found = api.snapshot_document(make_snapshot())["digest"]
    assert found["at"] == "06:45" and found["target"] == '["host","digest-settings"]'
    assert [(row["id"], row["kind"], row["enabled"], row["last_good_run"]) for row in found["sources"]] == [
        ("work", "program", False, None), ("code", "connector", True, "2026-11-17T06:00:00Z")]
    assert [a["key"] for a in found["actions"]] == ["I", "J"] and found["connect"]["target"] == '["host","connectors"]'


def test_Snapshot_Digest_BrokenFile_ShowsAFixedSentenceWithNoPath(capsys):
    from sakura.digest import config

    config.config_path().write_text("at = [")
    config.config_path().chmod(0o600)
    document = api.snapshot_document(make_snapshot())
    assert document["digest"]["problem"] == "the digest settings cannot be read"
    assert str(config.config_path().parent) not in json.dumps(document)
    assert str(config.config_path().parent) in capsys.readouterr().err


def test_Snapshot_Connectors_BrokenFile_ShowsAFixedSentenceWithNoPath(capsys):
    from sakura.connectors import registry

    registry.path().write_text("not json")
    document = api.snapshot_document(make_snapshot())
    assert document["connectors"] == [] and document["connectors_problem"] == "the connectors cannot be read"
    assert str(registry.path().parent) not in json.dumps(document)


# -- automations (E8)

AUTOMATIONS = host_actions.AUTOMATIONS


def test_Automation_OnOff_BuildsEnableOrDisable():
    on = argv_of("A", AUTOMATIONS, {"name": "wake-lanes", "on": "yes"})
    off = argv_of("A", AUTOMATIONS, {"name": "wake-lanes", "on": "no"})
    assert on[1:3] == ["automations", "enable"] and on[-2:] == ["--", "wake-lanes"]
    assert off[1:3] == ["automations", "disable"]


def test_Automation_FromThePhone_IsLocalOnly(tmp_path):
    snapshot = make_snapshot()
    controller = Controller(b"s" * 32, lambda: snapshot, lambda: snapshot, run=Spawns(), log_path=tmp_path / "log.jsonl")
    fields = {"key": "A", "target": api.target_id(AUTOMATIONS), "input.name": "wake-lanes", "input.on": "yes"}
    outcome = controller.perform(fields, overlay_actor(PHONE), proof_field="confirm", proof_source="overlay")
    assert isinstance(outcome, Refused) and outcome.reason == REMOTE_REFUSAL


def _proposed_wait():
    from sakura.automations import registry
    from sakura.cockpit import wait_actions
    from sakura.flow import waits

    registry.propose("wake-lanes", "Wake lanes", "every 15 minutes", "flow wake", "master:s1", NOW)
    (wait,) = [found for found in waits.collect([]) if found.kind == waits.AUTOMATION]
    return wait_actions.target_of(wait)


def test_AutomationWait_AcceptAndReject_RunTheAutomationsCommandWithTheShownDigest():
    target = _proposed_wait()
    accept, reject = argv_of("y", target, {}), argv_of("N", target, {})
    assert accept[1:3] == ["automations", "accept"] and accept[-3:-1] == ["--", "wake-lanes"] and len(accept[-1]) == 12
    assert reject[1:3] == ["automations", "reject"]
    assert [a.key for a in actions.applicable(target)] == ["y", "N", "read", "unread", "snooze"]


def test_Snapshot_Automations_ListsDefinitionsAndTheAlwaysOnServices():
    from sakura.automations import registry
    from sakura.digest import launchd

    pending = registry.propose("wake-lanes", "Wake lanes", "every 15 minutes", "flow wake", "master:s1", NOW)
    registry.accept("wake-lanes", pending.digest, launchd.Agent(launchctl=lambda arguments: 0,
                                                                 folder=keeper.state_path().parent / "agents",
                                                                 home=keeper.state_path().parent), NOW)
    document = api.snapshot_document(make_snapshot())
    names = [row["name"] for row in document["automations"]["items"]]
    assert names == ["wake-lanes", "keeper", "phone-page"]
    assert document["automations"]["target"] == '["host","automations"]'
    assert [a["key"] for a in document["automations"]["actions"]] == ["A"]
    assert document["waits"] == []


def test_Snapshot_AutomationProposal_IsAnApprovalWithAutomationSubkind():
    target = _proposed_wait()
    document = api.snapshot_document(make_snapshot(waits=[target.wait]))
    (wait,) = document["waits"]
    assert (wait["kind"], wait["subkind"]) == ("approval", "automation")
    assert [a["key"] for a in wait["actions"]] == ["y", "N", "read", "unread", "snooze"]


# -- give priority to the person's own sessions (E7)

ENGINE = host_actions.ENGINE


def test_Yield_OnOff_BuildsTheYieldCommand():
    on, off = argv_of("Y", ENGINE, {"on": "yes"}), argv_of("Y", ENGINE, {"on": "no"})
    assert on[1] == "yield" and on[-2:] == ["--", "on"] and off[-2:] == ["--", "off"]
    with pytest.raises(actions.ActionRefused):
        actions.plan("Y", ENGINE, {"on": "maybe"}, OWNER)


def test_Snapshot_Engine_YieldFieldsFollowTheSwitchAndTheLanesThatWait(repo):
    from sakura.flow import store, yield_person
    from test_cockpit_actions import flow_item

    lane = store.TaskStore(repo, "a")
    lane.create({"task": "a", "status": "running", "workspaces": {}})
    item = flow_item(repo, "a")
    found = api.snapshot_document(make_snapshot(items=[item]))["engine"]
    assert found["yield_to_person"] is False and found["yielding"] is False
    assert found["target"] == '["host","engine"]' and "Y" in [a["key"] for a in found["actions"]]
    yield_person.set_enabled(True)
    lane.update_state(lambda state: state.update({yield_person.YIELD_KEY: {"waiting": "person", "since": "2026-11-18T09:00:00Z"}}))
    found = api.snapshot_document(make_snapshot(items=[item]))["engine"]
    assert found["yield_to_person"] is True and found["yielding"] is True


# -- what changes what runs on the Mac is local only, remote refusals are made before the target is looked up

def _controller(tmp_path, snapshot):
    return Controller(b"s" * 32, lambda: snapshot, lambda: snapshot, run=Spawns(), log_path=tmp_path / "log.jsonl")


def _perform(controller, actor, key, target, **inputs):
    fields = {"key": key, "target": api.target_id(target) if hasattr(target, "name") else target,
              **{f"input.{name}": value for name, value in inputs.items()}}
    return controller.perform(fields, actor, proof_field="confirm", proof_source="overlay")


def _perform_local(controller, key, target, **inputs):
    """The host's own face (`sakura cockpit act`): the transport, not the actor's claim, decides what is local."""
    fields = {"key": key, "target": api.target_id(target) if hasattr(target, "name") else target,
              **{f"input.{name}": value for name, value in inputs.items()}}
    return controller.perform(fields, actors.Actor("app", "owner"), proof_field="confirm", proof_source="cli")


@pytest.mark.parametrize(("key", "inputs"), [("B", {"name": "keeper", "on": "yes"}), ("B", {"name": "keeper", "on": "no"}),
                                             ("B", {"name": "phone-page", "on": "yes"})])
def test_Services_OverlayActorRefused_LocalActorRuns(tmp_path, key, inputs):
    controller = _controller(tmp_path, make_snapshot())
    assert isinstance(_perform(controller, overlay_actor(PHONE), key, SERVICES, **inputs), Refused)
    local = _perform_local(controller, key, SERVICES, **inputs)
    assert not isinstance(local, Refused) and local.result.exit_code == 0


@pytest.mark.parametrize("on", ["yes", "no"])
def test_AutomationSwitch_OverlayActorRefused_LocalActorRuns(tmp_path, on):
    controller = _controller(tmp_path, make_snapshot())
    remote = _perform(controller, overlay_actor(PHONE), "A", host_actions.AUTOMATIONS, name="wake-lanes", on=on)
    assert isinstance(remote, Refused) and remote.reason == REMOTE_REFUSAL
    local = _perform_local(controller, "A", host_actions.AUTOMATIONS, name="wake-lanes", on=on)
    assert not isinstance(local, Refused) and local.result.exit_code == 0


def test_AutomationAccept_OverlayActorRefusedBeforeTheTargetIsLookedUp_LocalActorIsAsked(tmp_path):
    target = _proposed_wait()
    snapshot = make_snapshot(waits=[target.wait])
    controller = _controller(tmp_path, snapshot)
    remote = _perform(controller, overlay_actor(PHONE), "y", api.target_id(target))
    assert isinstance(remote, Refused) and remote.reason == REMOTE_REFUSAL
    unknown = _perform(controller, overlay_actor(PHONE), "y", '["wait","/","ghost","automation","ghost@abc"]')
    assert unknown.reason == REMOTE_REFUSAL  # the answer says nothing of what exists
    local = _perform_local(controller, "y", api.target_id(target))
    assert not isinstance(local, Refused)  # a confirmation is asked, as for any accept


def test_AutomationAndProposalRejectAndProposalAccept_StayAllowedFromThePhone(tmp_path):
    target = _proposed_wait()
    controller = _controller(tmp_path, make_snapshot(waits=[target.wait]))
    outcome = _perform(controller, overlay_actor(PHONE), "N", api.target_id(target))
    assert not isinstance(outcome, Refused)  # rejecting installs nothing


def test_Snapshot_Digest_EverySourceHasANameAndWhatItReads():
    from sakura.digest import config

    config.save(config.parse({"source": [
        {"id": "work-tickets", "run": ["/opt/work.sh"], "label": "Work tickets", "reads": "Open tickets assigned to you"},
        {"id": "ops-board", "run": ["/opt/ops.sh"]}, {"id": "code", "connector": "github"},
        {"id": "news", "feed": "https://example.org/feed"}]}))
    found = api.snapshot_document(make_snapshot())["digest"]["sources"]
    assert [(row["label"], row["reads"]) for row in found] == [
        ("Work tickets", "Open tickets assigned to you"), ("Ops board", ""),
        ("GitHub", "Pull requests and issues that wait on you"), ("News", "New entries of example.org")]
