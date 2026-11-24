import pytest

from sakura import layers, profile


class LayerError(ValueError):
    pass


def _drops_to_false(key, old, new):
    return old is True and new is False


def test_merge_rejects_relax_without_consent():
    with pytest.raises(LayerError, match="strict"):
        layers.merge({"strict": True}, [("local", {"strict": False})], _drops_to_false, LayerError)


def test_merge_records_relaxation_with_origins():
    values, origins, relaxations = layers.merge(
        {"strict": True, "other": 1},
        [("shared", {"other": 2}), ("local", {"strict": False, "relax": ["strict"]})],
        _drops_to_false,
        LayerError,
    )
    assert values == {"strict": False, "other": 2}
    assert origins == {"strict": "local", "other": "shared"}
    assert relaxations == ["strict (defaults to local)"]


def test_merge_rejects_unknown_key_and_unknown_relax():
    with pytest.raises(LayerError, match="unknown key"):
        layers.merge({"a": 1}, [("local", {"b": 1})], _drops_to_false, LayerError)
    with pytest.raises(LayerError, match="unknown relaxed"):
        layers.merge({"a": 1}, [("local", {"relax": ["b"]})], _drops_to_false, LayerError)


def test_profile_ignores_flow_table_in_local_overlay(repo):
    (repo / ".sakura").mkdir()
    (repo / ".sakura/local.toml").write_text('[flow]\npreset = "standard"\n')
    assert profile.load(repo, home=repo / "nohome").relaxations == []


def _never(key, old, new):
    return False


def _only_in_local(layer_name, key, old, new):
    return layer_name == "local" and new == "x"


def test_merge_asks_the_layer_aware_rule_with_the_name_of_the_layer():
    defaults = {"key": "base"}
    values, _, relaxations = layers.merge(defaults, [("personal", {"key": "x"})], _never, LayerError,
                                          relaxes_in_layer=_only_in_local)
    assert values == {"key": "x"} and relaxations == []
    with pytest.raises(LayerError, match="local layer relaxes key"):
        layers.merge(defaults, [("local", {"key": "x"})], _never, LayerError, relaxes_in_layer=_only_in_local)
    _, _, relaxations = layers.merge(defaults, [("local", {"key": "x", "relax": ["key"]})], _never, LayerError,
                                     relaxes_in_layer=_only_in_local)
    assert relaxations == ["key (defaults to local)"]
