"""The engine's rule controls (`K` set a rule, `U` use a preset, `V` save the rules as a preset) and the snapshot's `engine`."""

import json
from pathlib import Path

import pytest
from sakura import engine_home
from sakura.cockpit import actions, api, host_actions
from sakura.flow import presets, rules
from test_cockpit_actions import PROGRAM, Spawns, flow_item, invoke
from test_cockpit_snapshot import make_snapshot

ENGINE = host_actions.ENGINE
DOC = Path(__file__).resolve().parents[1] / "docs" / "cockpit-api.md"


def test_EngineTarget_IsTheFixedHostTargetOfTheRuleActions():
    assert ENGINE.key == ("host", "engine")
    assert sorted(action.key for action in actions.applicable(ENGINE)) == ["E", "F", "K", "M", "Q", "U", "V", "Y", "Z", "g", "j", "setupapply"]
    assert actions.applicable(host_actions.DIGEST) == [actions.ACTIONS["G"]]


@pytest.mark.parametrize(("key", "inputs", "tail"), [
    ("K", {"rule": "reviews.plan_review.on", "value": "false"}, ["engine", "set", "--", "reviews.plan_review.on", "false"]),
    ("U", {"preset": "lean"}, ["preset", "use", "--", "lean"]),
    ("V", {"name": "fast-fix"}, ["preset", "save", "--", "fast-fix"]),
])
def test_RuleActions_BuildTheEngineCommandWithTheActorBeforeTheSeparator(key, inputs, tail):
    _, spawns = invoke(key, ENGINE, inputs)
    (argv, _), = spawns.calls
    assert [part for part in argv if not part.startswith("--actor=")] == [PROGRAM, *tail]
    assert argv[argv.index("--") - 1].startswith("--actor=")


@pytest.mark.parametrize("key", ["K", "U", "V"])
def test_RuleActions_NoOtherTargetTakesThem(key, tmp_path):
    with pytest.raises(actions.ActionRefused, match="does not apply"):
        invoke(key, flow_item(tmp_path, "t1"), {})


def test_RuleActions_WrongInputs_AreRefusedBeforeAnythingRuns():
    with pytest.raises(actions.ActionRefused, match="missing value"):
        invoke("K", ENGINE, {"rule": "lanes"})
    with pytest.raises(actions.ActionRefused, match="invalid rule"):
        invoke("K", ENGINE, {"rule": "lanes; rm -rf", "value": "3"})


def test_RuleActions_ArePressedOnTheOwnersMachineOnly():
    assert {"K", "U", "V"} <= actions.LOCAL_ONLY_KEYS


def test_RuleAction_ARefusedRuleIsTheReasonTheClientSees():
    spawns = Spawns(stdout="lanes must be a whole number from 1 to 6; got '99'\n", returncode=2)
    result, _ = invoke("K", ENGINE, {"rule": "lanes", "value": "99"}, spawns=spawns)
    assert actions.refusal("K", result) == "lanes must be a whole number from 1 to 6; got '99'"


def test_Snapshot_Engine_NamesThePresetTheEditsAndTheControls():
    document = api.snapshot_document(make_snapshot())["engine"]
    assert document["preset"] == "standard" and document["edited"] is False
    assert document["id"] == '["host","engine"]' and document["key"] == ["host", "engine"]
    assert sorted(action["key"] for action in document["actions"]) == ["E", "F", "K", "M", "Q", "U", "V", "Y", "Z", "g", "j", "setupapply"]
    rules.set("subagents", "4", None)
    assert api.snapshot_document(make_snapshot())["engine"]["edited"] is True
    presets.choose("lean")
    assert api.snapshot_document(make_snapshot())["engine"]["preset"] == "lean"


def test_Snapshot_Engine_AConfigThatDoesNotLoadStillAnswers():
    (engine_home.root() / "config.toml").write_text("this is [not toml")
    document = api.snapshot_document(make_snapshot())["engine"]
    assert document["preset"] == "" and document["edited"] is False


def test_ApiDoc_DescribesThePresetPipelineRules():
    doc = DOC.read_text()
    for word in ("engine.preset", "engine.edited", "`K`", "`U`", "`V`", "sakura engine rules", "rules.toml"):
        assert word in doc, word


def test_Snapshot_Engine_ListsThePresetsWithTheChosenOneAndHidesTheLegacyOnes():
    listed = api.snapshot_document(make_snapshot())["engine"]["presets"]
    assert "vanilla" not in [entry["name"] for entry in listed]
    assert [entry["name"] for entry in listed if entry["chosen"]] == ["standard"]
    assert {"name", "label", "description", "chosen"} <= set(listed[0])


def test_Snapshot_Engine_ThePresetStaysChosenWhileItsRulesAreEdited():
    rules.set("subagents", "4", None)
    engine = api.snapshot_document(make_snapshot())["engine"]
    assert engine["edited"] is True
    assert [entry["name"] for entry in engine["presets"] if entry["chosen"]] == ["standard"]


def test_Snapshot_Engine_ALegacyPresetThePersonChoseIsStillListedAsChosen():
    presets.choose("vanilla")
    listed = api.snapshot_document(make_snapshot())["engine"]["presets"]
    assert [entry["name"] for entry in listed if entry["chosen"]] == ["vanilla"]


def test_Snapshot_Engine_AConfigThatDoesNotLoadStillListsThePresetsWithNoneChosen():
    (engine_home.root() / "config.toml").write_text("this is [not toml")
    listed = api.snapshot_document(make_snapshot())["engine"]["presets"]
    assert listed and not any(entry["chosen"] for entry in listed)


def test_Snapshot_Engine_AnUnusablePresetFileIsListedWithAFixedSentenceAndNoPath():
    folder = engine_home.root() / "presets"
    folder.mkdir()
    (folder / "broken.toml").write_text("this is [not toml")
    listed = {entry["name"]: entry for entry in api.snapshot_document(make_snapshot())["engine"]["presets"]}
    assert listed["broken"]["description"] == "this preset file cannot be read"
    assert str(engine_home.root()) not in json.dumps(listed)


def _defined_keys() -> list[str]:
    """Read the keys declared by the cockpit action factories before they enter the table."""
    import ast

    track_declarations = ast.parse(Path(actions.track_actions.__file__).read_text())
    specs = next(node.value for node in track_declarations.body
                 if isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id == "SPECS"
                                                        for target in node.targets))
    keys = [getattr(actions.track_actions, key.id) for key in specs.keys]
    for path in sorted((Path(actions.__file__).parent).glob("*.py")):
        for node in ast.walk(ast.parse(path.read_text())):
            name = getattr(node.func, "id", "") if isinstance(node, ast.Call) else ""
            if name not in ("Action", "_account_action", "_attention_action", "_scheduled_action") or not node.args:
                continue
            first = node.args[0]
            if isinstance(first, ast.Constant):
                keys.append(first.value)
            elif name == "_scheduled_action" and isinstance(first, ast.Attribute):
                keys.append(getattr(actions.scheduled_definition, first.attr))
    return keys


def test_Actions_NoKeyIsDefinedTwice():
    keys = _defined_keys()
    assert sorted(key for key in set(keys) if keys.count(key) > 1) == []
    assert len(keys) == len(actions.ACTIONS)  # a key defined twice would leave the table one short


def test_Snapshot_Engine_ListsEveryActionOfTheEngineTarget():
    engine_keys = {"K", "U", "V", "Q", "Y", "M", "F", "Z", "E", "g", "j", "setupapply"}
    assert {action["key"] for action in api.snapshot_document(make_snapshot())["engine"]["actions"]} == engine_keys


def test_HostActions_EveryHostActionBelongsToExactlyOneHostTarget():
    host_keys = {key for key, action in actions.ACTIONS.items() if host_actions.KIND_HOST in action.kinds}
    owners = {key: [target.name for target in host_actions.TARGETS if host_actions.allows(target, key)] for key in host_keys}
    assert {key: names for key, names in owners.items() if len(names) != 1} == {}
