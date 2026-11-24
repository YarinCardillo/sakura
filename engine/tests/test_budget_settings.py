import pytest

from sakura import engine_home
from sakura.budget import settings
from sakura.flow import config


@pytest.fixture
def home(tmp_path):
    return tmp_path / "home"


def _write(repo, name, text):
    (repo / ".sakura").mkdir(exist_ok=True)
    (repo / ".sakura" / name).write_text(text)


def _project(repo, text):
    """The per-project host layer: budget settings are the host's, and loosening one there needs relax."""
    path = engine_home.project_config_file(repo)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def test_budget_defaults(repo, home):
    loaded = config.load(repo, home=home)
    assert {key: value for key, value in loaded.values["budget"].items() if key != "routes"} == {
        "account": "", "reserve_week_pct": 25, "reserve_five_hour_pct": 30, "warn_flow_week_pct": 5,
        "ask_flow_week_pct": 10, "outside_share_alert": 0.4, "night": "23:00-07:00",
        "snapshot_max_age_minutes": 30}
    assert settings.load(loaded.values) == settings.BudgetSettings(
        account="", reserve_week_pct=25, reserve_five_hour_pct=30, warn_flow_week_pct=5,
        ask_flow_week_pct=10, outside_share_alert=0.4, night="23:00-07:00", snapshot_max_age_minutes=30)


def test_a_repository_layer_may_not_set_the_budget(repo, home):
    _write(repo, "flow.toml", "[budget]\nreserve_week_pct = 40\n")
    with pytest.raises(config.FlowConfigError, match="budget.reserve_week_pct may only be set by the host"):
        config.load(repo, home=home)


def test_a_host_layer_needs_relax_to_lower_the_reserve(repo, home):
    _project(repo, "[budget]\nreserve_week_pct = 10\n")
    with pytest.raises(config.FlowConfigError, match="budget.reserve_week_pct"):
        config.load(repo, home=home)
    _project(repo, "relax = [\"budget.reserve_week_pct\"]\n[budget]\nreserve_week_pct = 10\n")
    assert config.load(repo, home=home).values["budget"]["reserve_week_pct"] == 10
    _project(repo, "[budget]\nreserve_week_pct = 40\nreserve_five_hour_pct = 50\n")
    stricter = config.load(repo, home=home)
    assert stricter.values["budget"]["reserve_week_pct"] == 40 and stricter.relaxations == []


@pytest.mark.parametrize("key,relaxing,tightening", [
    ("reserve_five_hour_pct", 20, 35),
    ("warn_flow_week_pct", 6, 4),
    ("ask_flow_week_pct", 11, 9),
    ("outside_share_alert", 0.5, 0.3),
    ("snapshot_max_age_minutes", 45, 15),
])
def test_every_budget_threshold_has_a_stricter_direction(repo, home, key, relaxing, tightening):
    _project(repo, f"[budget]\n{key} = {relaxing}\n")
    with pytest.raises(config.FlowConfigError, match=f"budget.{key}"):
        config.load(repo, home=home)
    _project(repo, f"[budget]\n{key} = {tightening}\n")
    assert config.load(repo, home=home).relaxations == []


def test_account_and_night_are_not_thresholds(repo, home):
    _project(repo, '[budget]\naccount = "work"\nnight = "00:00-06:00"\n')
    loaded = config.load(repo, home=home)
    assert loaded.relaxations == []
    assert settings.load(loaded.values).account == "work"


@pytest.mark.parametrize("assignment,message", [
    ("reserve_week_pct = 101", "budget.reserve_week_pct"),
    ("reserve_five_hour_pct = -1", "budget.reserve_five_hour_pct"),
    ("outside_share_alert = 1.5", "budget.outside_share_alert"),
    ("outside_share_alert = true", "budget.outside_share_alert"),
    ('night = "25:00-07:00"', "budget.night"),
    ('night = "23:00-23:00"', "budget.night"),
    ('night = "23:00"', "budget.night"),
    ("snapshot_max_age_minutes = 0", "budget.snapshot_max_age_minutes"),
    ("snapshot_max_age_minutes = 1.5", "budget.snapshot_max_age_minutes"),
    ("account = 3", "budget.account"),
    ("warn_flow_week_pct = 4\nask_flow_week_pct = 3", "budget.warn_flow_week_pct"),
])
def test_invalid_budget_values_name_the_key(repo, home, assignment, message):
    _project(repo, f"[budget]\n{assignment}\n")
    with pytest.raises(config.FlowConfigError, match=message):
        config.load(repo, home=home)


def test_load_rejects_a_malformed_values_dict_with_a_clear_error():
    with pytest.raises(settings.BudgetSettingsError, match="budget.reserve_week_pct must be a number"):
        settings.load({"budget": {**config.DEFAULTS["budget"], "reserve_week_pct": "lots"}})


def test_effective_reserve_prefers_the_account_value_over_the_default():
    assert settings.effective_reserve(40.0, 25.0) == 40.0
    assert settings.effective_reserve(0.0, 25.0) == 0.0
    assert settings.effective_reserve(None, 25.0) == 25.0
    assert settings.effective_reserve(None, None) is None
