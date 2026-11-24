import pytest
from sakura.budget import predict
from sakura.budget.history import Sample
from sakura.budget.routing import Route
from sakura.flow.plan import BuildTask


def build(task_class, model_class, files, weighted, kind="build"):
    return Sample(kind, task_class, model_class, files, 0, 0, weighted)


def role_sample(role, model_class, weighted):
    return Sample(role, None, model_class, 0, 0, 0, weighted)


def spread(task_class, model_class, per_file, count=6):
    """`count` one-file samples whose per-file cost runs from per_file up in steps of ten percent."""
    return [build(task_class, model_class, 1, per_file * (1 + step / 10)) for step in range(count)]


def test_prediction_scales_with_files():
    samples = spread("mechanical", "sonnet", 1000)
    one = predict.task(predict.TaskShape("mechanical", 1), "sonnet", samples)
    four = predict.task(predict.TaskShape("mechanical", 4), "sonnet", samples)
    assert four.weighted == pytest.approx(4 * one.weighted)
    assert four.low == pytest.approx(4 * one.low)
    assert four.high == pytest.approx(4 * one.high)
    assert one.low <= one.weighted <= one.high
    assert one.n == 6


def test_samples_are_compared_per_changed_file():
    samples = [build("mechanical", "sonnet", files, files * 500.0) for files in (2, 5, 10, 3, 7)]
    prediction = predict.task(predict.TaskShape("mechanical", 6), "sonnet", samples)
    assert prediction.weighted == pytest.approx(3000)


def test_nearest_samples_fall_back_by_class():
    samples = [*spread("mechanical", "sonnet", 100), *spread("design", "sonnet", 900), *spread("design", "opus", 5000),
               build(None, "opus", 1, 7000, kind="pr")]
    same_class = predict.task(predict.TaskShape("mechanical", 1), "sonnet", samples)
    same_model = predict.task(predict.TaskShape("integration", 1), "sonnet", samples)
    anything = predict.task(predict.TaskShape("integration", 1), "haiku", samples)
    assert (same_class.n, same_model.n, anything.n) == (6, 12, len(samples))
    assert same_class.weighted < same_model.weighted < anything.weighted
    assert "mechanical sonnet" in same_class.basis and "any class sonnet" in same_model.basis
    assert "all classes and models" in anything.basis


def test_a_class_with_fewer_than_the_minimum_samples_defers_to_a_broader_level():
    samples = [build("mechanical", "sonnet", 1, 100.0), *spread("design", "sonnet", 900)]
    prediction = predict.task(predict.TaskShape("mechanical", 1), "sonnet", samples)
    assert prediction.n == 7
    assert "few samples" not in prediction.basis


def test_few_samples_widen_the_band():
    samples = [build("mechanical", "sonnet", 1, 1000.0)] * 3
    prediction = predict.task(predict.TaskShape("mechanical", 2), "sonnet", samples)
    assert prediction.weighted == pytest.approx(2000)
    assert prediction.low == pytest.approx(1000)
    assert prediction.high == pytest.approx(4000)
    assert "few samples" in prediction.basis and prediction.n == 3


def test_no_history_is_a_zero_prediction_not_an_error():
    prediction = predict.task(predict.TaskShape("mechanical", 3), "sonnet", [])
    assert (prediction.weighted, prediction.low, prediction.high, prediction.n) == (0, 0, 0, 0)
    assert prediction.basis == "no history"


def test_role_samples_are_not_task_samples():
    prediction = predict.task(predict.TaskShape("mechanical", 1), "sonnet", [role_sample("planner", "sonnet", 9e6)])
    assert prediction.basis == "no history"


def tasks_and_routes():
    tasks = [BuildTask("T1", "one", [], [], ["a.py", "b.py"], False, task_class="mechanical"),
             BuildTask("T2", "two", ["T1"], [], ["c.py"], False, task_class="design")]
    routes = {"T1": Route("sonnet", "", "", False, "mechanical"), "T2": Route("opus", "", "", True, "design")}
    return tasks, routes


def role_samples():
    return [role_sample(role, model, weighted) for role, model, weighted in (
        ("planner", "opus", 50_000.0), ("plan_reviewer", "sonnet", 8_000.0), ("code_reviewer", "sonnet", 6_000.0),
        ("evaluator", "opus", 30_000.0), ("reporter", "sonnet", 4_000.0))] * 5


def test_flow_prediction_sums_roles():
    tasks, routes = tasks_and_routes()
    roles = {"planner": "opus", "plan_reviewer": "sonnet", "code_reviewer": "sonnet", "evaluator": "opus",
             "reporter": "sonnet"}
    samples = [*spread("mechanical", "sonnet", 1000), *spread("design", "opus", 4000), *role_samples()]
    result = predict.flow(tasks, routes, roles, samples)
    assert result.by_class["sonnet"].weighted == pytest.approx(2 * 1250 + 8_000 + 2 * 6_000 + 4_000)
    assert result.by_class["opus"].weighted == pytest.approx(1 * 5000 + 50_000 + 30_000)
    assert result.missing == ()
    assert all(band.low <= band.weighted <= band.high for band in result.by_class.values())


def test_flow_prediction_names_what_has_no_history():
    tasks, routes = tasks_and_routes()
    roles = {"planner": "opus", "evaluator": "opus"}
    samples = [sample for sample in role_samples() if sample.kind in roles]
    result = predict.flow(tasks, routes, roles, samples)
    assert result.by_class == {"opus": predict.Band(80_000.0, 80_000.0, 80_000.0)}
    assert set(result.missing) == {"T1", "T2", "plan_reviewer", "code_reviewer", "reporter"}
    assert all(name in result.basis for name in result.missing)


def test_flow_prediction_without_any_history_is_empty_and_never_raises():
    tasks, routes = tasks_and_routes()
    result = predict.flow(tasks, routes, {}, [])
    assert result.by_class == {}
    assert "no history" in result.basis
