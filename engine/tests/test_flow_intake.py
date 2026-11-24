import hashlib
import os
import stat

import pytest

from sakura.flow import engine, exits
from sakura.flow.phases import intake
from conftest import git
from flow_support import FakeRunner, make_context

VALID_SPEC = "# Spec\n\n## Goal\nx\n\n## Journeys\n1. open the page\n"


def _write_spec(ctx, text=VALID_SPEC):
    def behaviour(request):
        ctx.store.spec.write_text(text)

    return behaviour


def _ctx(repo, behaviour=None, **kwargs):
    runner = FakeRunner()
    # Intake runs the planner in workspace 0 of the task's own repository, made from the base commit.
    base = {"base_commit": git(repo, "rev-parse", "HEAD"), "branch": "feat/flow-demo", "workspaces": {}}
    kwargs["state"] = {**base, **kwargs.get("state", {})}
    ctx = make_context(repo, runner=runner, **kwargs)
    runner.behaviour = behaviour(ctx) if behaviour else None
    (repo / "specs").mkdir(exist_ok=True)
    (repo / "specs/demo.md").write_text("build a thing\n")
    return ctx, runner


def test_intake_copies_file_ref_and_runs_planner(repo):
    ctx, runner = _ctx(repo, _write_spec)
    outcome = intake.run(ctx)
    assert (ctx.store.input).read_text() == "build a thing\n"
    assert outcome.kind == "await" and outcome.gate == "spec"
    request = runner.requests[0]
    assert (request.role, request.phase) == ("planner", "intake")
    assert request.values["input_path"] == str(ctx.store.input)


def test_intake_requires_journeys_heading(repo):
    ctx, runner = _ctx(repo, lambda c: _write_spec(c, "# Spec\n\nno journeys\n"))
    outcome = intake.run(ctx)
    assert outcome.kind == "blocked" and "## Journeys" in outcome.reason
    assert len(runner.requests) == 2
    assert "## Journeys" in runner.requests[1].values["previous_error"]
    assert runner.requests[0].values["previous_error"] == ""


def test_intake_rejects_empty_spec(repo):
    ctx, runner = _ctx(repo, lambda c: _write_spec(c, "  \n"))
    assert intake.run(ctx).kind == "blocked"


def test_intake_reruns_once_with_error_then_accepts(repo):
    attempts = []

    def behaviour(ctx):
        def run(request):
            attempts.append(1)
            ctx.store.spec.write_text("nothing" if len(attempts) == 1 else VALID_SPEC)
        return run

    ctx, runner = _ctx(repo, behaviour)
    assert intake.run(ctx).kind == "await"
    assert len(runner.requests) == 2


def test_intake_keeps_valid_existing_spec_without_rerun(repo):
    ctx, runner = _ctx(repo, _write_spec)
    ctx.store.spec.write_text(VALID_SPEC)
    assert intake.run(ctx).kind == "await"
    assert runner.requests == []


def test_intake_missing_file_blocks(repo):
    ctx, runner = _ctx(repo, _write_spec, state={"ref": "specs/missing.md"})
    outcome = intake.run(ctx)
    assert outcome.kind == "blocked" and "specs/missing.md" in outcome.reason
    assert runner.requests == []


def _approve(ctx):
    digest = hashlib.sha256(ctx.store.spec.read_bytes()).hexdigest()
    ctx.store.update_state(lambda s: s["approvals"].update(spec={"at": "now", "sha256": digest}))


def test_approved_spec_moves_on(repo):
    ctx, _ = _ctx(repo, _write_spec)
    intake.run(ctx)
    _approve(ctx)
    assert intake.run(ctx).kind == "next"


def test_spec_change_after_approval_requires_new_approval(repo):
    ctx, runner = _ctx(repo, _write_spec)
    intake.run(ctx)
    _approve(ctx)
    ctx.store.spec.write_text(VALID_SPEC + "\n## Extra\nchanged\n")
    outcome = intake.run(ctx)
    assert outcome.kind == "await" and outcome.gate == "spec"
    assert len(runner.requests) == 1


def test_approve_spec_false_skips_gate(repo):
    ctx, _ = _ctx(repo, _write_spec, overrides={"approve_spec": False, "relax": ["approve_spec"]})
    assert intake.run(ctx).kind == "next"


def test_github_source_fetches_issue_with_gh(repo, tmp_path, monkeypatch):
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    gh = bin_dir / "gh"
    # The engine gives `gh` an environment made for it, so the script names its output file itself.
    gh.write_text(f'#!/bin/sh\necho "$@" > {tmp_path / "args"}\nprintf \'{{"title":"T","body":"B","url":"U"}}\'\n')
    gh.chmod(gh.stat().st_mode | stat.S_IXUSR)
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")
    ctx, _ = _ctx(repo, _write_spec, state={"ref": "42"}, overrides={"spec": {"source": "github"}})
    assert intake.run(ctx).kind == "await"
    assert (tmp_path / "args").read_text().split() == ["issue", "view", "42", "--json", "title,body,url"]
    text = ctx.store.input.read_text()
    assert "T" in text and "B" in text and "U" in text


def test_notion_source_passes_ref_to_agent(repo):
    ctx, runner = _ctx(repo, _write_spec, state={"ref": "https://notion.so/page"}, overrides={"spec": {"source": "notion"}})
    intake.run(ctx)
    assert "https://notion.so/page" in runner.requests[0].values["input_hint"]
    assert not ctx.store.input.exists()


def test_intake_registered_for_the_engine():
    import sakura.flow.phases  # noqa: F401

    assert engine._HANDLERS["intake"] is intake.run


def test_malformed_gh_output_is_a_clear_block(repo, tmp_path, monkeypatch):
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    gh = bin_dir / "gh"
    gh.write_text("#!/bin/sh\necho not-json\n")
    gh.chmod(gh.stat().st_mode | stat.S_IXUSR)
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")
    ctx, runner = _ctx(repo, _write_spec, state={"ref": "42"}, overrides={"spec": {"source": "github"}})
    outcome = intake.run(ctx)
    assert outcome.kind == "blocked" and "unexpected output" in outcome.reason and runner.requests == []


def test_intake_inputs_hash_follows_the_stored_capture_and_the_approval_never_the_workspace_file(repo):
    ctx, _ = _ctx(repo, _write_spec)
    intake.run(ctx)  # the task started before captures were stored: intake captures once and stores it
    first = intake.inputs_hash(ctx)
    (repo / "specs/demo.md").write_text("changed\n")
    assert intake.inputs_hash(ctx) == first
    ctx.store.update_state(lambda s: s["approvals"].update(spec={"at": "t", "sha256": "abc"}))
    assert intake.inputs_hash(ctx) != first


def test_intake_does_not_offer_a_spec_whose_planner_said_it_could_not_read_the_task(repo):
    ctx, runner = _ctx(repo, _write_spec)
    runner.result = {"error": "cannot read the task description"}
    outcome = intake.run(ctx)
    assert outcome.kind == "blocked" and "cannot read the task description" in outcome.reason


# -- the task file is read without following a link: a symlink to a secret never reaches engine storage -----------------


CANARY = "CANARY-SECRET-5f1c"


def _symlinked_task(repo, tmp_path):
    secret = tmp_path / "secret.txt"
    secret.write_text(f"{CANARY}\n")
    (repo / "specs").mkdir(exist_ok=True)
    (repo / "specs" / "task.md").symlink_to(secret)
    return secret


def test_TaskFile_ASymlinkToASecretIsRefusedAtIntakeAndNothingIsCopied(repo, tmp_path):
    _symlinked_task(repo, tmp_path)
    ctx, runner = _ctx(repo, _write_spec, state={"ref": "specs/task.md"})

    outcome = intake.run(ctx)

    assert outcome.kind == "blocked" and "specs/task.md" in outcome.reason
    assert not ctx.store.input.exists() or CANARY not in ctx.store.input.read_text()
    assert runner.requests == []


def test_TaskFile_AHardLinkToAnotherFileIsRefused(repo, tmp_path):
    secret = tmp_path / "secret.txt"
    secret.write_text(f"{CANARY}\n")
    (repo / "specs").mkdir(exist_ok=True)
    os.link(secret, repo / "specs" / "task.md")
    ctx, _ = _ctx(repo, _write_spec, state={"ref": "specs/task.md"})

    assert intake.run(ctx).kind == "blocked" and not ctx.store.input.exists()


def test_TaskFile_ASymlinkedFolderOnThePathIsRefused(repo, tmp_path):
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "task.md").write_text(f"{CANARY}\n")
    (repo / "linked").symlink_to(outside)
    ctx, _ = _ctx(repo, _write_spec, state={"ref": "linked/task.md"})

    assert intake.run(ctx).kind == "blocked" and not ctx.store.input.exists()


@pytest.mark.parametrize("ref", ["../secret.txt", "/etc/hosts"])
def test_TaskFile_APathOutsideTheRepositoryIsRefused(repo, tmp_path, ref):
    ctx, _ = _ctx(repo, _write_spec, state={"ref": ref})

    outcome = intake.run(ctx)

    assert outcome.kind == "blocked" and not ctx.store.input.exists()


def test_TaskFile_TheCapturedBytesAreWhatIsCopied(repo):
    (repo / "specs").mkdir(exist_ok=True)
    (repo / "specs" / "task.md").write_bytes(b"add a greeting\n\xc3\xa9\n")
    ctx, _ = _ctx(repo, _write_spec, state={"ref": "specs/task.md"})

    assert intake.run(ctx).kind == "await"
    assert ctx.store.input.read_bytes() == b"add a greeting\n\xc3\xa9\n"


def test_TaskFile_StartRefusesASymlinkedTaskFileBeforeCreatingAnyTask(repo, tmp_path, monkeypatch, capsys):
    from sakura import install
    from sakura.flow import cli, exits, store

    _symlinked_task(repo, tmp_path)
    install.install(repo)
    monkeypatch.chdir(repo)

    code = cli.main(["start", "specs/task.md"])

    assert code == exits.USAGE and "specs/task.md" in capsys.readouterr().out
    assert not store.flow_root(repo).exists() or not list(store.flow_root(repo).iterdir())


# -- the task file is captured once, with a size limit (review 6, finding 5) ---------------------------------------------


def _task_file(repo, content=b"build a thing\n"):
    (repo / "specs").mkdir(exist_ok=True)
    (repo / "specs" / "big.md").write_bytes(content)
    return "specs/big.md"


def test_TaskFile_OverTheLimit_IsRefusedAtStartAndAtIntake_WithTheLimitInTheMessage(repo, monkeypatch, capsys):
    from sakura import install
    from sakura.flow import cli, exits, tasks

    ref = _task_file(repo, b"x" * (tasks.MAX_TASK_FILE_BYTES + 1))
    install.install(repo)
    monkeypatch.chdir(repo)
    assert cli.main(["start", ref]) == exits.USAGE
    assert "larger than 1 MiB" in capsys.readouterr().out
    ctx, runner = _ctx(repo, _write_spec, state={"ref": ref})
    outcome = intake.run(ctx)
    assert outcome.kind == "blocked" and "larger than 1 MiB" in outcome.reason
    assert not ctx.store.input.exists() and runner.requests == []


def test_TaskFile_ASparseHugeFile_IsRefusedWithoutBeingRead(repo):
    (repo / "specs").mkdir(exist_ok=True)
    with open(repo / "specs" / "huge.md", "wb") as sparse:
        sparse.truncate(50 << 30)
    ctx, runner = _ctx(repo, _write_spec, state={"ref": "specs/huge.md"})
    outcome = intake.run(ctx)
    assert outcome.kind == "blocked" and "larger than" in outcome.reason and runner.requests == []


@pytest.mark.parametrize("mutation", ["grow", "swap", "rewrite"])
def test_TaskFile_ChangedDuringTheCapture_IsRefusedAndNothingIsCopied(repo, monkeypatch, mutation):
    ref = _task_file(repo)
    path = repo / ref

    def change():
        if mutation == "grow":
            path.open("ab").write(b"more\n")
        elif mutation == "rewrite":
            path.open("r+b").write(b"BUILD\n")
        else:
            (repo / "other.md").write_bytes(b"swapped file\n")
            os.rename(repo / "other.md", path)

    ctx, runner = _ctx(repo, _write_spec, state={"ref": ref})
    real = os.read
    done = []

    def read(descriptor, size):
        if not done:
            done.append(True)
            change()
        return real(descriptor, size)

    monkeypatch.setattr(os, "read", read)
    outcome = intake.run(ctx)
    assert outcome.kind == "blocked" and "changed while it was read" in outcome.reason
    assert not ctx.store.input.exists() and runner.requests == []


def test_TaskFile_OneCaptureFeedsTheHashTheValidationAndTheCopy(repo, monkeypatch):
    from sakura.flow import tasks

    ref = _task_file(repo, b"build once\n")
    ctx, _ = _ctx(repo, _write_spec, state={"ref": ref})
    reads = []
    real = tasks.beneath.read_capped
    monkeypatch.setattr(tasks.beneath, "read_capped", lambda *args: reads.append(args) or real(*args))
    assert intake.run(ctx).kind == "await"
    assert len(reads) == 1 and ctx.store.input.read_bytes() == b"build once\n"
    captured = tasks.capture_task_file(repo, ref)
    assert captured.sha256 == hashlib.sha256(b"build once\n").hexdigest() and captured.data == b"build once\n"


# -- the task file is captured once, when the task is created, and kept in engine storage (review 7, finding 3) ----------


def _started(repo, monkeypatch, content=b"build the original\n"):
    """`sakura flow start specs/task.md` with the engine run left out: the task as its creation leaves it."""
    from sakura import install
    from sakura.flow import cli, store

    (repo / "specs").mkdir(exist_ok=True)
    (repo / "specs" / "task.md").write_bytes(content)
    install.install(repo)
    monkeypatch.chdir(repo)
    monkeypatch.setattr(cli, "_advance", lambda *args, **kwargs: 0)
    assert cli.main(["start", "specs/task.md"]) == 0
    (task_id,) = [path.name for path in store.flow_root(repo).iterdir() if path.is_dir()]
    return store.TaskStore(repo, task_id)


def _context_of(repo, task_store, monkeypatch, behaviour=None):
    from sakura.flow import config, notify

    # The phase as the engine registers it, whatever other tests left in the registry.
    monkeypatch.setitem(engine._HANDLERS, "intake", intake.run)
    monkeypatch.setitem(engine._INPUTS, "intake", intake.inputs_hash)
    runner = FakeRunner()
    cfg = config.load(repo, home=repo / "nohome")
    ctx = engine.FlowContext(repo, task_store, cfg, runner, notify.LedgerNotifier(task_store), config.steps(cfg.values))
    runner.behaviour = behaviour(ctx) if behaviour else None
    return ctx, runner


def test_TaskFile_IsCapturedAtCreation_IntoEngineStorage_WithItsDigest(repo, monkeypatch):
    task_store = _started(repo, monkeypatch)
    assert task_store.input.read_bytes() == b"build the original\n"
    assert task_store.read_state()["input_sha256"] == hashlib.sha256(b"build the original\n").hexdigest()


def test_TaskFile_ReplacedInTheWorkspaceAfterStartAndDuringThePlanner_ChangesNothing(repo, monkeypatch):
    from sakura.flow import tasks

    task_store = _started(repo, monkeypatch)
    (repo / "specs" / "task.md").write_bytes(b"build the replacement\n")
    seen = []

    def planner(ctx):
        def run(request):
            seen.append(open(request.values["input_path"], "rb").read())
            (repo / "specs" / "task.md").write_bytes(b"build another replacement\n")
            ctx.store.spec.write_text(VALID_SPEC)
        return run

    ctx, runner = _context_of(repo, task_store, monkeypatch, planner)
    reads = []
    real = tasks.beneath.read_capped
    monkeypatch.setattr(tasks.beneath, "read_capped", lambda *args: reads.append(args) or real(*args))
    outcome = engine._run_phase(ctx, "intake")
    original = b"build the original\n"
    assert outcome.kind == "await" and seen == [original] and reads == []
    assert task_store.input.read_bytes() == original
    state = task_store.read_state()
    assert state["input_sha256"] == hashlib.sha256(original).hexdigest()
    intake.inputs_hash(ctx)
    assert reads == []
    approved = hashlib.sha256(ctx.store.spec.read_bytes()).hexdigest()
    ctx.store.update_state(lambda s: s["approvals"].update(spec={"at": "t", "sha256": approved}))
    assert engine._run_phase(ctx, "intake").kind == "next"
    recorded = task_store.read_state()["completed"]["intake"]
    assert recorded == hashlib.sha256(original + b"\0" + approved.encode()).hexdigest() and reads == []


def test_TaskFile_TheStoredCaptureChangedInEngineStorage_BlocksIntake(repo, monkeypatch):
    task_store = _started(repo, monkeypatch)
    task_store.input.write_bytes(b"not what was captured\n")
    ctx, runner = _context_of(repo, task_store, monkeypatch, _write_spec)
    outcome = intake.run(ctx)
    assert outcome.kind == "blocked" and "digest" in outcome.reason and runner.requests == []


def test_TaskText_IsStoredAtCreationWithItsDigestToo(repo, monkeypatch):
    from sakura import install
    from sakura.flow import cli, store

    install.install(repo)
    monkeypatch.chdir(repo)
    monkeypatch.setattr(cli, "_advance", lambda *args, **kwargs: 0)
    assert cli.main(["start", "-m", "add a greeting option"]) == 0
    (task_id,) = [path.name for path in store.flow_root(repo).iterdir() if path.is_dir()]
    task_store = store.TaskStore(repo, task_id)
    assert task_store.input.read_bytes() == b"add a greeting option\n"
    assert task_store.read_state()["input_sha256"] == hashlib.sha256(b"add a greeting option\n").hexdigest()


def test_intake_TheWriterEndsWithoutWritingTheSpec_SaysSoInPlainWordsWithItsOwnLastMessage(repo):
    ctx, runner = _ctx(repo)
    runner.final = "I could not tell what the task asks for."

    outcome = intake.run(ctx)

    assert outcome.kind == "blocked"
    assert outcome.reason == ("The planner ended without writing the spec. It said: \"I could not tell what the task asks "
                              "for.\" Resume the lane to ask it again, or stop it.")
    assert "spec.md was not written" in ctx.store.ledger.read_text()
