"""Completion runs from one immutable checkout: the app under test, check rows, evaluator rows and the full gate."""

import sys

import pytest

from conftest import git
from flow_support import FakeRunner, engine_head, make_context
from sakura.flow import checks, completion, config, contract, stack, task_repository
from sakura.flow.phases import evaluate, integrate

STEPS_OFF = config.PresetSteps(False, False, False)
STEPS_ON = config.PresetSteps(True, True, True)
# Serves the content of node_modules/answer.txt, or "missing": what an ignored dependency folder holds decides it.
SERVER = """import http.server, os, sys
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        path = os.path.join("node_modules", "answer.txt")
        body = open(path).read().strip() if os.path.exists(path) else "missing"
        self.send_response(200)
        self.end_headers()
        self.wfile.write(body.encode())
    def log_message(self, *_args):
        pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
"""
PROBE = """import os, sys, urllib.request
body = urllib.request.urlopen(os.environ["SAKURA_FLOW_BASE_URL"], timeout=5).read().decode()
print(body)
sys.exit(0 if body == "PASS" else 1)
"""
APP_ROW = f"{sys.executable} probe.py"
VENV_ROW = "test -e .venv/planted.py"
WORKSPACE_PORT = 42007


def _commit(repo, files):
    for name, text in files.items():
        (repo / name).parent.mkdir(parents=True, exist_ok=True)
        (repo / name).write_text(text)
    git(repo, "add", *files)
    git(repo, "commit", "-qm", "fixture")


def _row(row_id, flow, prover="check"):
    return contract.Row(row_id, "j", "s", "cli", None, flow, False, False, prover)


def _context(repo, rows, steps=STEPS_OFF, overrides=None):
    port = WORKSPACE_PORT
    state = {"workspaces": {"0": {"path": str(repo), "branch": "main", "ports": [port, port + 9]}},
             "contract_sha256": "", "base_commit": git(repo, "rev-parse", "HEAD"), "branch": "main",
             "approvals": {"checks": {"commands": sorted({row.flow for row in rows})}}}
    app = {"start": f"{sys.executable} server.py $SAKURA_FLOW_PORT", "ready_seconds": 15,
           "ready_url": "http://127.0.0.1:$SAKURA_FLOW_PORT/"}
    ctx = make_context(repo, own_repository=True, state=state, overrides={"app": app, **(overrides or {})})
    object.__setattr__(ctx, "steps", steps)
    contract.save(ctx.store, rows)
    engine_head(ctx)
    return ctx


def _plant(repo):
    """Ignored files in the live workspace that would make every row pass if anything used them."""
    (repo / "node_modules").mkdir()
    (repo / "node_modules" / "answer.txt").write_text("PASS\n")
    (repo / ".venv").mkdir()
    (repo / ".venv" / "planted.py").write_text("PLANTED = True\n")


def _receipt(ctx, row_id):
    return ctx.store.read_state()[checks.RECEIPTS_KEY][f"{row_id}/api"]


@pytest.fixture
def project(repo):
    _commit(repo, {".gitignore": "node_modules/\n.venv/\n", "server.py": SERVER, "probe.py": PROBE})
    return repo


def test_CheckRow_APlantedIgnoredVenvOrNodeModulesInTheLiveWorkspace_DoesNotInfluenceTheReceipt(project):
    _plant(project)
    ctx = _context(project, [_row("R1", APP_ROW), _row("R2", VENV_ROW)])
    evaluate.run(ctx)
    assert _receipt(ctx, "R1")["exit"] != 0, "the app under test read the live workspace's node_modules"
    assert _receipt(ctx, "R2")["exit"] != 0, "the row read the live workspace's .venv"
    assert (project / "node_modules" / "answer.txt").read_text() == "PASS\n"


def test_CheckRow_ACommitASurvivorAddsToWorkspaceZero_IsNeitherRunNorReceipted(project):
    # Review 11, finding 1: the rows run on the engine's head and their receipts name it, whatever the branch holds.
    ctx = _context(project, [_row("R1", "test ! -e survivor.txt")])
    head = git(project, "rev-parse", "HEAD")
    _commit(project, {"survivor.txt": "never reviewed\n"})
    evaluate.run(ctx)
    receipt = _receipt(ctx, "R1")
    assert receipt["exit"] == 0 and receipt["revision"] == head


def test_CompletionSetup_InstallsIntoTheBuildFolder_AndTheAppUnderTestUsesIt(project):
    ctx = _context(project, [_row("R1", APP_ROW)], overrides={"workspace": {"setup": ["echo PASS > node_modules/answer.txt"]}})
    evaluate.run(ctx)
    receipt = _receipt(ctx, "R1")
    assert receipt["exit"] == 0 and receipt["revision"] == git(project, "rev-parse", "HEAD")
    assert not (project / "node_modules").exists(), "setup ran in the checkout, not in the live workspace"
    for folder in (project.parent, task_repository.workspace_root(ctx.config.values, ctx.repo)):
        assert not [path for path in folder.iterdir() if path.name.startswith(completion.CHECKOUT_PREFIX)]


def test_Evaluator_TheAppAndTheRowToolShareTheEnginesCheckout(project, monkeypatch):
    seen = {}

    def started(_ctx, workspace, checkout=None):
        seen["app"] = checkout.source if checkout else workspace.path
        return _Running()

    def evaluator(request):
        seen["recorded"] = completion.recorded(ctx.store.read_state()).source

    monkeypatch.setattr(stack, "start", started)
    ctx = _context(project, [_row("R1", "true", "evaluator")], STEPS_ON)
    object.__setattr__(ctx, "runner", FakeRunner(evaluator))
    evaluate._evaluate(ctx, project)
    assert seen["app"] == seen["recorded"] and seen["app"].name.startswith(completion.CHECKOUT_PREFIX)
    assert completion.recorded(ctx.store.read_state()) is None, "the record goes with the checkout"


def test_RunRow_TheFinalEvaluationWithoutTheEnginesCheckout_IsRefused(project, monkeypatch, capsys):
    row = _row("R1", "true", "evaluator")
    ctx = _context(project, [row], STEPS_ON)
    monkeypatch.delenv("SAKURA_FLOW_STACK", raising=False)
    monkeypatch.setenv("SAKURA_FLOW_PHASE", "evaluate")
    monkeypatch.chdir(project)
    assert evaluate.run_row(ctx, row) == 1
    assert "no completion checkout" in capsys.readouterr().err
    checks.ingest_row_receipts(ctx.store)
    assert checks.EVALUATOR_RECEIPTS_KEY not in ctx.store.read_state()


# -- the full-test gate ---------------------------------------------------------------------------------------


CONFTEST = """import pathlib
APP = pathlib.Path(__file__).parent / "app.py"
ORIGINAL = APP.read_text()
APP.write_text("VALUE = 2\\n")


def pytest_sessionfinish(session, exitstatus):
    APP.write_text(ORIGINAL)
"""


def test_FullTests_AConftestThatRewritesTrackedFiles_CannotPassTheGate(repo):
    _commit(repo, {"app.py": "VALUE = 1\n", "conftest.py": CONFTEST,
                   "tests/test_app.py": "import app\n\n\ndef test_value():\n    assert app.VALUE == 2\n"})
    ctx = _context(repo, [], overrides={"tests": {"full": [f"{sys.executable} -m pytest -q -p no:cacheprovider tests"]},
                                        "app": {"start": ""}})
    outcome = integrate.run_full_tests(ctx)
    assert outcome is not None and outcome.kind == "blocked"
    assert ctx.store.read_state()["full_green_head"] is None
    assert "Operation not permitted" in _full_log(ctx), "the rewrite itself must be what the cage refused"
    assert (repo / "app.py").read_text() == "VALUE = 1\n"


def test_FullTests_APlantedIgnoredFileInTheLiveWorkspace_DoesNotTurnTheGateGreen(project):
    _commit(project, {"tests/test_dep.py": "import os\n\n\ndef test_dep():\n    assert os.path.exists('node_modules/answer.txt')\n"})
    _plant(project)
    ctx = _context(project, [], overrides={"tests": {"full": [f"{sys.executable} -m pytest -q -p no:cacheprovider tests"]},
                                           "app": {"start": ""}})
    assert integrate.run_full_tests(ctx).kind == "blocked"
    assert ctx.store.read_state()["full_green_head"] is None
    assert "1 failed" in _full_log(ctx)


def _full_log(ctx):
    return (ctx.store.operations_dir / integrate.FULL_LOG).read_text()


def test_FullTests_GreenOnTheCheckout_RecordTheCapturedRevision(project):
    _commit(project, {"tests/test_ok.py": "def test_ok():\n    assert True\n"})
    ctx = _context(project, [], overrides={"tests": {"full": [f"{sys.executable} -m pytest -q -p no:cacheprovider tests"]},
                                           "app": {"start": ""}})
    assert integrate.run_full_tests(ctx) is None
    assert ctx.store.read_state()["full_green_head"] == git(project, "rev-parse", "HEAD")


def test_FullTests_AStrayFileOrACommitInTheLiveWorkspace_IsNotWhatTheyRunOn(project):
    # Review 11: the full tests run on the engine's head; nothing of workspace 0 (a stray file, a survivor's commit)
    # takes part, so neither blocks nor feeds them.
    ctx = _context(project, [], overrides={"tests": {"full": ["test ! -e stray.txt && test ! -e moved.txt"]},
                                           "app": {"start": ""}})
    head = git(project, "rev-parse", "HEAD")
    _commit(project, {"moved.txt": "a survivor's commit\n"})
    (project / "stray.txt").write_text("uncommitted\n")
    assert integrate.run_full_tests(ctx) is None
    assert ctx.store.read_state()["full_green_head"] == head


class _Running:
    base_url = "http://127.0.0.1:1"

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        return None
