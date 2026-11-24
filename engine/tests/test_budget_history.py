import json

import pytest
from sakura.budget import history
from sakura.flow import store
from sakura.usage.archive import store as archive

PLAN = {"tasks": [
    {"id": "T1", "title": "one", "depends_on": [], "rows": ["R1", "R2"], "files": ["a.py", "b.py", "c.py"],
     "seam": False, "task_class": "mechanical"}]}


def result_log(path, output, cache_read=0):
    usage = {"input_tokens": 100, "cache_creation_input_tokens": 10, "cache_read_input_tokens": cache_read,
             "output_tokens": output}
    path.write_text(json.dumps({"type": "result", "usage": usage, "total_cost_usd": 0.1}) + "\n")


def run_record(name, role, model, cwd, state="finished"):
    return {"name": name, "role": role, "phase": role, "provider": "claude", "model": model, "state": state,
            "cwd": str(cwd), "started_at": "2026-10-20T10:00:00+00:00", "ended_at": "2026-10-20T10:05:00+00:00"}


@pytest.fixture
def flow_repo(repo, tmp_path):
    """One finished task: two builder runs in the workspace of T1, a planner run and a live reviewer run."""
    task = store.TaskStore(repo, "demo")
    workspace = tmp_path / "ws-1"
    workspace.mkdir()
    runs = {"build-1": run_record("build-1", "builder", "sonnet", workspace),
            "build-2": run_record("build-2", "builder", "sonnet", workspace),
            "plan-1": run_record("plan-1", "planner", "claude-opus-5-5", repo),
            "review-1": run_record("review-1", "code_reviewer", "sonnet", workspace, state="running")}
    task.create({"task": "demo", "status": "done", "workspaces": {"1": {"path": str(workspace), "branch": "b"}},
                 "build": {"T1": {"workspace": "1"}}, "runs": runs})
    task.plan_json.write_text(json.dumps(PLAN))
    task.runs_dir.mkdir(parents=True, exist_ok=True)
    result_log(task.runs_dir / "build-1.log", 1000)
    result_log(task.runs_dir / "build-2.log", 500, cache_read=10_000)
    result_log(task.runs_dir / "plan-1.log", 200)
    result_log(task.runs_dir / "review-1.log", 50)
    return repo


def test_flow_samples_sum_the_builder_runs_of_a_task(flow_repo):
    found = {sample.kind: sample for sample in history.flow_samples([flow_repo])}
    build = found["build"]
    shape = (build.task_class, build.model_class, build.files, build.rows, build.lines)
    assert shape == ("mechanical", "sonnet", 3, 2, 0)
    assert build.weighted == pytest.approx((100 + 20 + 5000) + (100 + 20 + 1000 + 2500))


def test_flow_samples_keep_other_roles_as_their_own_kind_and_skip_live_runs(flow_repo):
    found = {sample.kind: sample for sample in history.flow_samples([flow_repo])}
    assert set(found) == {"build", "planner"}
    assert (found["planner"].task_class, found["planner"].model_class) == (None, "opus")
    assert found["planner"].weighted == pytest.approx(100 + 20 + 1000)


def test_flow_samples_without_repos_read_nothing():
    assert history.flow_samples([]) == []


def test_a_task_with_an_unreadable_plan_still_yields_its_role_samples(flow_repo):
    store.TaskStore(flow_repo, "demo").plan_json.write_text("not json")
    assert {sample.kind for sample in history.flow_samples([flow_repo])} == {"planner"}


def add_turn(conn, key, ts, model, output, branch="feature/x", cwd="/w/widgets"):
    conn.execute("INSERT INTO turns VALUES ('claude', ?, 'a', 's', NULL, ?, ?, ?, ?, 0, 0, 0, ?, NULL)",
                 (key, ts, cwd, branch, model, output))


def add_pr(conn, number, branch, files, merged="2026-10-21T00:00:00Z"):
    conn.execute("INSERT INTO pull_requests VALUES ('acme/widgets', ?, ?, 'main', ?, 30, 12, 1, '2026-10-20T00:00:00Z',"
                 " NULL, ?, NULL, 0, '[]', 't')", (number, branch, files, merged))


def test_pr_samples_are_merged_pull_requests_with_their_turns(tmp_path):
    conn = archive.open_db(tmp_path / "usage.db")
    add_pr(conn, 1, "feature/x", 4)
    add_pr(conn, 2, "feature/open", 3, merged=None)
    add_pr(conn, 3, "feature/none", 5)
    add_turn(conn, "a", "2026-10-20T12:00:00+00:00", "claude-opus-5-5", 100)
    add_turn(conn, "b", "2026-10-20T13:00:00+00:00", "claude-sonnet-5-5", 40)
    add_turn(conn, "c", "2026-10-20T13:00:00+00:00", "claude-sonnet-5-5", 40, branch="feature/open")
    conn.commit()
    found = history.pr_samples(conn)
    assert found == [history.Sample("pr", None, "opus", 4, 42, 0, 700.0)]


def test_samples_merges_both_sources_and_defaults_to_no_flow_repos(tmp_path, flow_repo):
    conn = archive.open_db(tmp_path / "usage.db")
    add_pr(conn, 1, "feature/x", 4)
    add_turn(conn, "a", "2026-10-20T12:00:00+00:00", "claude-opus-5-5", 100)
    conn.commit()
    assert [sample.kind for sample in history.samples(conn)] == ["pr"]
    assert {sample.kind for sample in history.samples(conn, [flow_repo])} == {"pr", "build", "planner"}


def test_flow_runs_name_the_session_of_each_recorded_claude_run(flow_repo):
    task = store.TaskStore(flow_repo, "demo")
    task.update_state(lambda state: state["runs"]["plan-1"].update(session_id="s-plan"))
    task.update_state(lambda state: state["runs"]["build-1"].update(provider="codex", session_id="s-codex"))

    assert history.flow_runs([flow_repo]) == [history.Run(flow_repo.name, "demo", "planner", "s-plan")]
    assert history.flow_runs([]) == []
