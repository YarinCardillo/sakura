import json
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from sakura import engine_home
from sakura import cli
from sakura.budget import weekly, weekly_window
from sakura.flow import store
from sakura.usage.archive import calibration, command, settings
from sakura.usage.archive import store as archive
from sakura.usage.archive.calibration import Ratio

START = datetime(2026, 10, 20, 8, 0, tzinfo=timezone.utc)
END = datetime(2026, 10, 27, 8, 0, tzinfo=timezone.utc)
NOW = datetime(2026, 10, 28, 9, 0, tzinfo=timezone.utc)
RESET = "2026-10-27T08:00:00Z"
PLAN = settings.Plan("a", "max20x", datetime(2026, 1, 1, tzinfo=timezone.utc))
# Weighted tokens per 1% of the week: opus spends the quota fastest.
PER_PCT = {"opus": 500.0, "sonnet": 1000.0, "haiku": 4000.0}


@pytest.fixture
def usage(tmp_path):
    return settings.Settings(db=tmp_path / "usage.db", sources=(), pr_repos=(), pr_days=7, plans=(PLAN,))


@pytest.fixture
def conn(usage):
    return archive.open_db(usage.db)


@pytest.fixture(autouse=True)
def calibrated(monkeypatch):
    def ratios(conn, plans, window="seven_day"):
        return [Ratio("a", "max20x", name, per_pct, 6, 6.0, window) for name, per_pct in PER_PCT.items()]

    monkeypatch.setattr(calibration, "compute", ratios)


def at(hours):
    return (START + timedelta(hours=hours)).isoformat()


def turn(conn, key, hours, model, output, session="s-int", cwd="/w/widgets", account="a", branch=None):
    conn.execute("INSERT INTO turns VALUES ('claude', ?, ?, ?, NULL, ?, ?, ?, ?, 0, 0, 0, ?, NULL)",
                 (key, account, session, at(hours), cwd, branch, model, output))


def snapshot(conn, hours, pct, resets=RESET, account="a"):
    conn.execute("INSERT INTO quota_snapshots (account, ts, subscription_type, status, five_hour_pct,"
                 " five_hour_resets_at, seven_day_pct, seven_day_resets_at)"
                 " VALUES (?, ?, 'max', 'allowed', 1.0, NULL, ?, ?)", (account, at(hours), pct, resets))


def flow_repo_with_run(repo, session_id, role="planner"):
    task = store.TaskStore(repo, "demo")
    record = {"name": f"{role}-1", "role": role, "phase": role, "provider": "claude", "model": "opus",
              "state": "finished", "cwd": str(repo), "session_id": session_id,
              "started_at": "2026-10-20T10:00:00+00:00", "ended_at": "2026-10-20T10:05:00+00:00"}
    task.create({"task": "demo", "status": "done", "runs": {record["name"]: record}})
    return repo


def figures(rows):
    return {row.name: (row.figure.tokens, round(row.figure.pct, 2)) for row in rows}


def test_weekly_groups_by_project_role_and_model(conn, usage, repo):
    cwds = [("/w/widgets", "widgets")]
    conn.executemany("INSERT INTO cwds VALUES (?, ?, 't')", cwds)
    turn(conn, "1", 1, "claude-opus-5-5", 200, session="s-flow")
    turn(conn, "2", 1.5, "claude-sonnet-5", 100)
    turn(conn, "3", 2, "claude-sonnet-5", 100, session="s-lane", cwd="/lanes/g/lane-1/app")
    turn(conn, "4", -1, "claude-opus-5-5", 999)
    turn(conn, "5", 200, "claude-opus-5-5", 999)
    turn(conn, "6", 1, "claude-opus-5-5", 999, account="other")
    snapshot(conn, 0, 0.0)
    snapshot(conn, 4, 5.0)
    conn.commit()
    flow_repo_with_run(repo, "s-flow")

    report = weekly.build(conn, usage, START, END, account="a", flow_repos=[repo], lane_roots=[Path("/lanes/g")])

    assert figures(report.by_project) == {"widgets": (1500.0, 2.5), "app": (500.0, 0.5)}
    assert figures(report.by_role) == {"planner": (1000.0, 2.0), "interactive": (500.0, 0.5), "lane": (500.0, 0.5)}
    assert figures(report.by_class) == {"opus": (1000.0, 2.0), "sonnet": (1000.0, 1.0)}
    assert (report.total.tokens, round(report.total.pct, 2), report.plan) == (2000.0, 3.0, "max20x")
    assert report.total.basis == "max20x calibration"
    assert [row.name for row in report.by_class] == ["opus", "sonnet"], "largest first, ties by name"


def test_the_project_without_a_resolved_repo_is_the_last_part_of_its_cwd(conn, usage):
    turn(conn, "1", 1, "claude-sonnet-5", 100, cwd="/somewhere/deep/gadget")
    turn(conn, "2", 1, "claude-sonnet-5", 100, cwd=None)

    report = weekly.build(conn, usage, START, END, account="a")

    assert set(figures(report.by_project)) == {"gadget", "unknown"}


def test_the_outside_share_is_the_rise_local_turns_do_not_explain(conn, usage):
    turn(conn, "1", 1, "claude-opus-5-5", 200)
    turn(conn, "2", 2, "claude-sonnet-5", 100)
    snapshot(conn, 0, 0.0)
    snapshot(conn, 4, 5.0)
    conn.commit()

    outside = weekly.build(conn, usage, START, END, account="a").outside

    assert (outside.rise, round(outside.pct, 2), round(outside.share, 2)) == (5.0, 2.5, 0.5)
    assert (outside.low, outside.high) == (1.5, 3.5), "plan percentages have 1% resolution"
    assert "calibrated" in outside.basis


def test_without_calibration_every_figure_says_so(conn, usage, monkeypatch):
    monkeypatch.setattr(calibration, "compute", lambda conn, plans, window="seven_day": [])
    turn(conn, "1", 1, "claude-opus-5-5", 200)
    snapshot(conn, 0, 0.0)
    snapshot(conn, 4, 5.0)
    conn.commit()

    report = weekly.build(conn, usage, START, END, account="a")

    assert report.total.pct is None and report.total.basis == "no calibration"
    assert report.by_project[0].figure.tokens == 1000.0 and report.by_project[0].figure.basis == "no calibration"
    assert report.outside.pct is None and report.outside.basis == "no calibration"


def test_the_band_comes_from_the_spread_of_the_calibrated_ratios(conn, usage, monkeypatch):
    monkeypatch.setattr(calibration, "compute", lambda conn, plans, window="seven_day": [
        Ratio("a", "max20x", "opus", 500.0, 6, 6.0, window, (400.0, 625.0))])
    turn(conn, "1", 1, "claude-opus-5-5", 200)

    total = weekly.build(conn, usage, START, END, account="a").total

    assert (total.low, total.pct, total.high) == (1000 / 625, 2.0, 1000 / 400)


def add_pr(conn, number, branch):
    conn.execute("INSERT INTO pull_requests VALUES ('acme/widgets', ?, ?, 'main', 3, 30, 12, 1,"
                 " '2026-10-19T00:00:00Z', NULL, NULL, NULL, 0, '[]', 't')", (number, branch))


def test_top_three_with_cheaper_route(conn, usage, repo):
    conn.execute("INSERT INTO cwds VALUES ('/w/widgets', 'widgets', 't')")
    turn(conn, "1", 1, "claude-opus-5-5", 600, session="s-flow")
    for number, model, output in ((7, "claude-opus-5-5", 400), (8, "claude-sonnet-5", 300), (9, "claude-haiku-4-5", 20)):
        add_pr(conn, number, f"feature/{number}")
        turn(conn, f"pr{number}", 2, model, output, session=f"s{number}", branch=f"feature/{number}")
    conn.commit()
    flow_repo_with_run(repo, "s-flow")

    top = weekly.build(conn, usage, START, END, account="a", flow_repos=[repo]).top_tasks

    assert [(task.kind, task.name, task.figure.tokens) for task in top] == [
        ("flow", f"{repo.name}/demo", 3000.0), ("pr", "acme/widgets#7", 2000.0), ("pr", "acme/widgets#8", 1500.0)]
    flow, opus_pr, sonnet_pr = (task.cheaper for task in top)
    assert (flow.steps, flow.pct_now, flow.pct_cheaper, flow.saving_pct) == ((("opus", "sonnet"),), 6.0, 3.0, 3.0)
    assert (opus_pr.pct_now, opus_pr.pct_cheaper) == (4.0, 2.0)
    assert (sonnet_pr.steps, sonnet_pr.pct_now, sonnet_pr.pct_cheaper, sonnet_pr.saving_pct) == (
        (("sonnet", "haiku"),), 1.5, 0.375, 1.125)
    assert flow.basis == "max20x calibration"


def test_a_flow_session_counts_for_its_task_not_for_a_pull_request(conn, usage, repo):
    add_pr(conn, 7, "feature/7")
    turn(conn, "1", 1, "claude-opus-5-5", 600, session="s-flow", branch="feature/7", cwd="/w/widgets")
    conn.execute("INSERT INTO cwds VALUES ('/w/widgets', 'widgets', 't')")
    conn.commit()
    flow_repo_with_run(repo, "s-flow")

    top = weekly.build(conn, usage, START, END, account="a", flow_repos=[repo]).top_tasks

    assert [task.kind for task in top] == ["flow"]


def test_a_haiku_only_task_has_no_cheaper_route(conn, usage):
    conn.execute("INSERT INTO cwds VALUES ('/w/widgets', 'widgets', 't')")
    add_pr(conn, 9, "feature/9")
    turn(conn, "1", 2, "claude-haiku-4-5", 20, branch="feature/9")
    conn.commit()

    assert weekly.build(conn, usage, START, END, account="a").top_tasks[0].cheaper is None


def test_the_cheaper_route_without_a_calibrated_target_class_says_so(conn, usage, monkeypatch):
    monkeypatch.setattr(calibration, "compute", lambda conn, plans, window="seven_day": [
        Ratio("a", "max20x", "opus", 500.0, 6, 6.0, window)])
    conn.execute("INSERT INTO cwds VALUES ('/w/widgets', 'widgets', 't')")
    add_pr(conn, 7, "feature/7")
    turn(conn, "1", 2, "claude-opus-5-5", 400, branch="feature/7")
    conn.commit()

    cheaper = weekly.build(conn, usage, START, END, account="a").top_tasks[0].cheaper

    assert (cheaper.pct_now, cheaper.pct_cheaper, cheaper.saving_pct) == (4.0, None, None)


# -- windows -----------------------------------------------------------------------------------------------------


def test_windows_are_the_weekly_resets_of_the_snapshots(conn):
    snapshot(conn, 1, 3.0)
    snapshot(conn, 2, 4.0, resets="2026-10-27T08:00:00+00:00")
    snapshot(conn, 3, 1.0, resets="2026-11-03T08:00:00Z")
    snapshot(conn, 3, 9.0, resets="2026-11-03T08:00:00Z", account="b")

    found = weekly_window.windows(conn, "a")

    assert found == [weekly_window.Window(START, END), weekly_window.Window(END, END + timedelta(days=7))]


def test_the_default_window_is_the_last_completed_one(conn):
    snapshot(conn, 1, 3.0)
    snapshot(conn, 3, 1.0, resets="2026-11-03T08:00:00Z")

    assert weekly_window.pick_window(conn, "a", None, NOW) == weekly_window.Window(START, END)
    assert weekly_window.pick_window(conn, "a", None, START) is None


def test_week_selects_the_window_starting_in_that_iso_week(conn):
    snapshot(conn, 1, 3.0)
    snapshot(conn, 3, 1.0, resets="2026-11-03T08:00:00Z")
    utc = timezone.utc
    first, second = weekly_window.Window(START, END), weekly_window.Window(END, END + timedelta(days=7))

    assert weekly_window.pick_window(conn, "a", "2026-W43", NOW, utc) == first, "starts Tue 20 Oct, the page's week"
    assert weekly_window.pick_window(conn, "a", "2026-W44", NOW, utc) == second
    assert weekly_window.pick_window(conn, "a", "2026-W45", NOW, utc) is None
    assert weekly_window.pick_window(conn, "a", "2026-W42", NOW, utc) is None


def test_a_running_window_can_be_picked_by_week_but_is_not_the_default(conn):
    snapshot(conn, 3, 1.0, resets="2026-11-03T08:00:00Z")
    during = END + timedelta(days=2)

    assert weekly_window.pick_window(conn, "a", None, during) is None
    assert weekly_window.pick_window(conn, "a", "2026-W44", during, timezone.utc).end > during


def test_reset_times_a_few_seconds_apart_are_one_window(conn):
    snapshot(conn, 1, 3.0, resets="2026-10-27T08:00:00Z")
    snapshot(conn, 2, 4.0, resets="2026-10-27T08:00:07Z")
    snapshot(conn, 3, 5.0, resets="2026-10-27T07:59:58Z")
    snapshot(conn, 4, 1.0, resets="2026-11-03T08:00:00Z")

    found = weekly_window.windows(conn, "a")

    assert [window.end for window in found] == [END + timedelta(seconds=7), END + timedelta(days=7)]


def test_a_malformed_week_is_refused():
    for text in ("2026-43", "2026-W54", "26-W01", "2026-W00"):
        with pytest.raises(ValueError, match="YYYY-Www"):
            weekly_window.parse_week(text)


def test_lane_roots_come_from_the_lane_config(tmp_path):
    assert weekly.lane_roots(tmp_path / "missing.toml") == ()
    config = tmp_path / "lanes.toml"
    config.write_text('[g]\nrepo = "/r"\nsession = "s"\nworktree_root = "/lanes/g"\nbranch = "lane/{name}"\n'
                      'base = "main"\nsetup = []\nup = "u"\ndown = "d"\nstatus = "st"\nagent = "claude"\n')
    assert weekly.lane_roots(config) == (Path("/lanes/g"),)


# -- the page ----------------------------------------------------------------------------------------------------


def owner_values(tmp_path):
    from sakura.flow import config as flow_config

    home = tmp_path / "isolated-home"
    engine_home.config_file().write_text(
        f'[report]\nowner_dir = "{tmp_path}/other/{{repo}}"\n\n[report.owner_dirs]\nsakura = "{tmp_path}/notes"\n')
    return flow_config.load_personal(home).values


def week_with_turns(conn):
    turn(conn, "1", 1, "claude-opus-5-5", 200)
    snapshot(conn, 0, 0.0)
    snapshot(conn, 4, 2.0)
    conn.commit()


def test_weekly_page_written_once_per_reset(conn, usage, tmp_path):
    week_with_turns(conn)
    values = owner_values(tmp_path)
    page = tmp_path / "notes" / "consumi-a-2026-W43.md"

    assert weekly.write_pages(conn, usage, NOW, values) == [f"weekly report a: {page.name}"]

    text = page.read_text()
    assert text.startswith('---\ntitle: "Consumi settimana 43"\nproject: sakura\nkind: report\naccount: a\n---\n')
    assert "max20x calibration" in text and "Per progetto" in text and "limite superiore" in text
    page.write_text("edited by the owner\n")
    assert weekly.write_pages(conn, usage, NOW, values) == []
    assert page.read_text() == "edited by the owner\n"
    assert sorted(path.name for path in page.parent.iterdir()) == [page.name], "no temp file is left"


def test_no_page_before_the_reset_or_for_a_week_without_turns(conn, usage, tmp_path):
    week_with_turns(conn)
    snapshot(conn, 5, 1.0, account="empty")
    conn.commit()
    values = owner_values(tmp_path)

    assert weekly.write_pages(conn, usage, END - timedelta(minutes=1), values) == []
    assert weekly.write_pages(conn, usage, NOW, values) == ["weekly report a: consumi-a-2026-W43.md"]
    assert not list((tmp_path / "notes").glob("*empty*"))


def test_a_next_reset_gets_its_own_page(conn, usage, tmp_path):
    week_with_turns(conn)
    values = owner_values(tmp_path)
    weekly.write_pages(conn, usage, NOW, values)
    turn(conn, "2", 24 * 7 + 5, "claude-opus-5-5", 100)
    snapshot(conn, 24 * 7 + 6, 1.0, resets="2026-11-03T08:00:00Z")
    conn.commit()

    lines = weekly.write_pages(conn, usage, END + timedelta(days=8), values)

    assert lines == ["weekly report a: consumi-a-2026-W44.md"]


def test_a_codex_account_gets_no_claude_weekly_page(conn, usage, tmp_path):
    week_with_turns(conn)
    snapshot(conn, 1, 1.0, account="codex:a")
    conn.commit()

    lines = weekly.write_pages(conn, usage, NOW, owner_values(tmp_path))

    assert lines == ["weekly report a: consumi-a-2026-W43.md"]


def test_an_account_that_is_not_a_file_name_is_refused(conn, usage, tmp_path):
    snapshot(conn, 1, 1.0, account="../x")
    conn.commit()

    with pytest.raises(ValueError, match="account"):
        weekly.write_pages(conn, usage, NOW, owner_values(tmp_path))


# -- the command and the archive run -----------------------------------------------------------------------------


def write_home_config(tmp_path):
    home = tmp_path / "isolated-home"
    (home / ".config/sakura").mkdir(parents=True, exist_ok=True)
    (home / ".config/sakura/usage.toml").write_text(
        '[[sources]]\nprovider = "claude"\naccount = "a"\nroot = "/nowhere/projects"\n')
    engine_home.config_file().write_text(
        f'[budget]\naccount = "a"\n\n[report.owner_dirs]\nsakura = "{tmp_path}/notes"\n')


def test_usage_weekly_prints_the_last_completed_week(conn, usage, tmp_path, monkeypatch, capsys):
    write_home_config(tmp_path)
    week_with_turns(conn)
    monkeypatch.setattr(settings, "load", lambda: usage)

    assert cli.main(["usage", "weekly"]) == 0

    out = capsys.readouterr().out
    assert "max20x calibration" in out and "by project" in out and "widgets" in out
    assert "upper bound" in out


def test_usage_weekly_json_and_week_flag(conn, usage, tmp_path, monkeypatch, capsys):
    write_home_config(tmp_path)
    week_with_turns(conn)
    monkeypatch.setattr(settings, "load", lambda: usage)

    assert cli.main(["usage", "weekly", "--week", "2026-W43", "--json"]) == 0

    data = json.loads(capsys.readouterr().out)
    assert data["account"] == "a" and data["start"] == START.isoformat() and data["total"]["basis"]


def test_usage_weekly_without_a_window_exits_1(conn, usage, tmp_path, monkeypatch, capsys):
    write_home_config(tmp_path)
    monkeypatch.setattr(settings, "load", lambda: usage)

    assert cli.main(["usage", "weekly"]) == 1
    assert "no completed weekly window" in capsys.readouterr().err


def test_a_bad_week_flag_is_a_usage_error(capsys):
    with pytest.raises(SystemExit) as stop:
        cli.main(["usage", "weekly", "--week", "43"])
    assert stop.value.code == 2 and "YYYY-Www" in capsys.readouterr().err


def test_weekly_lines_write_the_page_from_the_personal_config(conn, usage, tmp_path):
    write_home_config(tmp_path)
    week_with_turns(conn)

    assert command._weekly_lines(conn, usage, NOW) == ["weekly report a: consumi-a-2026-W43.md"]
    assert (tmp_path / "notes" / "consumi-a-2026-W43.md").is_file()


def test_weekly_lines_without_a_usage_config_do_nothing(conn, usage, tmp_path):
    week_with_turns(conn)

    assert command._weekly_lines(conn, usage, NOW) == []
    assert not (tmp_path / "notes").exists()


def test_the_text_and_the_page_carry_the_basis_of_every_figure(conn, usage, tmp_path):
    from sakura.budget import weekly_render

    week_with_turns(conn)
    report = weekly.build(conn, usage, START, END, account="a")

    for rendered in (weekly_render.text(report), weekly_render.page(report, 43)):
        assert rendered.count("max20x calibration") >= 4, "total, project, role and class each name their basis"


def test_usage_weekly_of_a_running_week_says_it_is_partial(conn, usage, tmp_path, capsys):
    from sakura.usage.archive import command as archive_command

    write_home_config(tmp_path)
    week_with_turns(conn)
    during = END - timedelta(days=1)

    assert archive_command.weekly_view(usage, "2026-W43", "a", False, during) == 0
    assert capsys.readouterr().out.startswith("week in progress, up to 2026-10-26 08:00 UTC\n")
    assert archive_command.weekly_view(usage, "2026-W43", "a", True, during) == 0
    assert json.loads(capsys.readouterr().out)["partial_until"] == during.isoformat()
    assert archive_command.weekly_view(usage, "2026-W43", "a", True, NOW) == 0
    assert json.loads(capsys.readouterr().out)["partial_until"] is None


def test_the_cheaper_route_carries_bands_and_its_basis(conn, usage, monkeypatch):
    from sakura.budget import weekly_render

    monkeypatch.setattr(calibration, "compute", lambda conn, plans, window="seven_day": [
        Ratio("a", "max20x", "opus", 500.0, 6, 6.0, window, (400.0, 625.0)),
        Ratio("a", "max20x", "sonnet", 1000.0, 6, 6.0, window, (800.0, 1250.0))])
    conn.execute("INSERT INTO cwds VALUES ('/w/widgets', 'widgets', 't')")
    add_pr(conn, 7, "feature/7")
    turn(conn, "1", 2, "claude-opus-5-5", 400, branch="feature/7")
    conn.commit()

    report = weekly.build(conn, usage, START, END, account="a")

    cheaper = report.top_tasks[0].cheaper
    assert (cheaper.now_band, cheaper.cheaper_band) == ((2000 / 625, 2000 / 400), (2000 / 1250, 2000 / 800))
    assert cheaper.saving_band == pytest.approx((2000 / 625 - 2000 / 800, 2000 / 400 - 2000 / 1250))
    for rendered in (weekly_render.text(report), weekly_render.page(report, 43)):
        assert "(3.2-5.0)" in rendered and "(1.6-2.5)" in rendered and "max20x calibration" in rendered.split("opus > sonnet")[1]
