import hashlib
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from sakura import install
from sakura.flow import checks, config, contract, exits, store
from sakura.flow.phases import integrate, pull_request
from conftest import git
from flow_support import engine_head, make_context


def _row(passes=True):
    return contract.Row("R1", "journey", "step", "api", None, "true", False, passes)


def _context(repo, rows, steps=None, names=("work.txt",), contents=None, task="demo", branch="feat/demo"):
    base = git(repo, "rev-parse", "HEAD")
    files = {**{name: "the task's change\n" for name in names}, **(contents or {})}
    for name, text in files.items():
        (repo / name).write_text(text)
    git(repo, "add", *files)
    git(repo, "commit", "-qm", "the task's change")
    state = {"base_commit": base, "branch": branch, "base_branch": "main", "contract_sha256": "",
             "full_green_head": git(repo, "rev-parse", "HEAD"), "workspaces": {"0": {"path": str(repo)}}, "pr": {},
             "push_url": "https://example.test/origin.git"}
    ctx = make_context(repo, task=task, state=state)
    engine_head(ctx)
    contract.save(ctx.store, rows)
    digest = hashlib.sha256(ctx.store.contract.read_bytes()).hexdigest()
    ctx.store.update_state(lambda current: current.update(contract_sha256=digest))
    ctx.store.spec.write_text("# Add greeting\n\n## Journeys\n")
    object.__setattr__(ctx, "steps", steps or config.PresetSteps(True, True, True))
    ctx.store.update_state(lambda current: current.update(
        full_green_inputs_sha256=integrate.full_tests_inputs_hash(ctx)))
    _receipts_at_head(ctx, repo, rows)
    return ctx


def _receipts_at_head(ctx, repo, rows):
    """The engine's evaluator receipt of a passing run at the delivery head for every green row, as delivery needs."""
    evidence = ctx.store.evidence_dir / "fixture.log"
    evidence.parent.mkdir(parents=True, exist_ok=True)
    evidence.write_text("exit 0\n")
    for row in rows:
        if row.passes:
            checks.record_receipt(ctx.store, row, contract.rows_digest(rows), git(repo, "rev-parse", "HEAD"), 0,
                                  evidence, checks.EVALUATOR_RECEIPTS_KEY)


def test_pr_blocked_when_rows_open(repo):
    ctx = _context(repo, [_row(False)])
    outcome = pull_request.run(ctx)
    assert outcome.kind == "blocked" and "contract rows" in outcome.reason


def test_pr_rejects_full_tests_from_old_configuration(repo):
    ctx = _context(repo, [_row()])
    ctx.config.values["tests"]["full"] = ["true"]

    outcome = pull_request.run(ctx)

    assert outcome.kind == "blocked"
    assert "current test configuration" in outcome.reason


def test_push_mismatch_blocks(repo, monkeypatch):
    ctx = _context(repo, [_row()])
    monkeypatch.setattr(pull_request, "_push", lambda *_args: None)
    monkeypatch.setattr(pull_request, "_remote_head", lambda *_args: "different")
    outcome = pull_request.run(ctx)
    assert outcome.kind == "blocked" and "push not verified" in outcome.reason


def test_pr_command_log_is_an_operation_log(repo, tmp_path):
    import sys

    ctx = _context(repo, [_row()])
    outcome, output, log = pull_request._command(
        ctx, repo, "pull-request-lookup", [sys.executable, "-c", "print('[]')"]
    )

    assert outcome.exit_code == 0 and output.strip() == "[]"
    assert log == ctx.store.runs_dir / "operations" / "pull-request-lookup.log"
    assert log.read_text().strip() == "[]"
    assert not list(ctx.store.runs_dir.glob("*.log"))


def test_resume_recovers_existing_draft_for_exact_head_and_base(repo, monkeypatch):
    ctx = _context(repo, [_row()])
    url = "https://example.test/pull/42"
    commands = []
    monkeypatch.setattr(pull_request, "_push", lambda *_args: None)
    monkeypatch.setattr(pull_request, "_remote_head", lambda _workspace, _branch, _url: git(repo, "rev-parse", "HEAD"))

    def command(_ctx, _cwd, _name, argv):
        commands.append(argv)
        result = SimpleNamespace(stopped=False, stalled=False, timed_out=False, exit_code=0)
        output = ('[{"url":"' + url + '","headRefOid":"' + git(repo, "rev-parse", "HEAD") +
                  '","baseRefName":"main","isDraft":true}]')
        return result, output, Path("pull-request.log")

    monkeypatch.setattr(pull_request, "_command", command)

    outcome = pull_request.run(ctx)

    assert outcome.kind == "next"
    assert ctx.store.read_state()["pr"] == {"url": url, "head": git(repo, "rev-parse", "HEAD")}
    assert not any(argv[2] == "create" for argv in commands)


def test_pr_body_verified():
    state = {"preset": "verified", "base_branch": "main"}
    body = pull_request.pr_body(state, [_row(), contract.Row("R2", "j", "s", "api", None, "true", True, True)],
                                config.PresetSteps(True, True, True), [], "flow/demo/report.md")
    assert "2/2 contract rows passed by the evaluator" in body
    assert "Report: flow/demo/report.md" in body


def test_pr_body_vanilla_states_not_verified_not_reviewed():
    state = {"preset": "vanilla", "base_branch": "main"}
    body = pull_request.pr_body(state, [_row(False)], config.PresetSteps(False, False, False), [], "report.md")
    assert "not verified by an evaluator (preset vanilla)" in body
    assert "not reviewed: plan review, code review" in body


def test_pr_body_lists_spec_rulings():
    state = {"preset": "verified", "base_branch": "main"}
    ruling = {"path": "tests/test_app.py", "kind": "test_change", "decision": "grant",
              "why": "requirement changed", "cost_if_wrong": "weak coverage", "spec_change": True}
    body = pull_request.pr_body(state, [_row()], config.PresetSteps(True, True, True), [ruling], "report.md")
    assert "pending owner confirmation" in body and "tests/test_app.py" in body


def test_after_sets_base_branch(repo, tmp_path, monkeypatch):
    from sakura.flow import cli

    install.install(repo)
    monkeypatch.chdir(repo)
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    parent = store.TaskStore(repo, "parent")
    parent.create({"task": "parent", "branch": "feat/parent"})
    git(repo, "checkout", "-qb", "feat/parent")
    (repo / "parent.txt").write_text("parent\n")
    git(repo, "add", "parent.txt")
    git(repo, "commit", "-qm", "parent")
    branch_head = git(repo, "rev-parse", "HEAD")
    monkeypatch.setattr(cli, "_advance", lambda *_args: exits.OK)
    code = cli.main(["start", "specs/child.md", "--after", "parent"])
    assert code == exits.OK
    state = store.TaskStore(repo, "child").read_state()
    assert state["base_branch"] == "feat/parent" and state["base_commit"] == branch_head


def _bare(path):
    subprocess.run(["git", "init", "-q", "--bare", str(path)], check=True, capture_output=True)
    return path


def test_TaskStart_RecordsThePushUrlOfTheRepositoryOrigin(repo, tmp_path):
    from sakura.flow import tasks

    origin = _bare(tmp_path / "origin.git")
    git(repo, "remote", "add", "origin", str(origin))
    state = tasks.initial_state(repo, config.load(repo), "demo", "specs/demo.md", {})
    assert state["push_url"] == str(origin)


def test_TaskStart_RecordsThePushUrlNotTheFetchUrlWhenTheyDiffer(repo, tmp_path):
    from sakura.flow import tasks

    fetch, push = _bare(tmp_path / "fetch.git"), _bare(tmp_path / "push.git")
    git(repo, "remote", "add", "origin", str(fetch))
    git(repo, "config", "remote.origin.pushurl", str(push))
    state = tasks.initial_state(repo, config.load(repo), "demo", "specs/demo.md", {})
    assert state["push_url"] == str(push)


def test_Push_UsesTheUrlRecordedAtStart_EvenIfTheRemoteConfigChangedAfterwards(repo, tmp_path):
    recorded, decoy = _bare(tmp_path / "recorded.git"), _bare(tmp_path / "decoy.git")
    git(repo, "remote", "add", "origin", str(recorded))
    ctx = _context(repo, [_row()])
    ctx.store.update_state(lambda state: state.update(push_url=str(recorded)))
    git(repo, "remote", "set-url", "origin", str(decoy))
    git(repo, "branch", "feat/demo")
    assert pull_request._push(ctx, repo, git(repo, "rev-parse", "HEAD"), "feat/demo", str(recorded)) is None
    pushed = subprocess.run(["git", "-C", str(recorded), "rev-parse", "refs/heads/feat/demo"], capture_output=True, text=True)
    assert pushed.returncode == 0 and pushed.stdout.strip() == git(repo, "rev-parse", "HEAD")
    assert subprocess.run(["git", "-C", str(decoy), "rev-parse", "refs/heads/feat/demo"], capture_output=True).returncode != 0


def test_PullRequest_WithoutARecordedPushUrl_BlocksBeforePushing(repo, monkeypatch):
    ctx = _context(repo, [_row()])
    ctx.store.update_state(lambda state: state.pop("push_url", None))
    monkeypatch.setattr(pull_request, "_push", lambda *_args: pytest.fail("pushed"))
    outcome = pull_request.run(ctx)
    assert outcome.kind == "blocked" and "push URL" in outcome.reason


@pytest.mark.parametrize("branch, base", [("main", "main"), ("master", "main"), ("develop", "develop"), ("trunk", "main")])
def test_PullRequest_RefusesToPushAProtectedBranch(repo, monkeypatch, branch, base):
    ctx = _context(repo, [_row()])
    ctx.store.update_state(lambda state: state.update(branch=branch, base_branch=base))
    monkeypatch.setattr(pull_request.profile, "remote_default_branch", lambda _repo: "trunk")
    monkeypatch.setattr(pull_request, "_push", lambda *_args: pytest.fail("pushed"))
    outcome = pull_request.run(ctx)
    assert outcome.kind == "blocked" and "protected branch" in outcome.reason


def test_RemoteDefaultBranch_IsReadFromTheRepositoryOrigin(repo):
    git(repo, "update-ref", "refs/remotes/origin/trunk", "HEAD")
    git(repo, "symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/trunk")
    assert pull_request.profile.remote_default_branch(repo) == "trunk"


def _push_blocked(repo, monkeypatch, branch):
    ctx = _context(repo, [_row()])
    ctx.store.update_state(lambda state: state.update(branch=branch))
    monkeypatch.setattr(pull_request, "_push", lambda *_args: pytest.fail("pushed"))
    return pull_request.run(ctx)


def test_PullRequest_RefusesToPushAProtectedBranchOfTheProfile(repo, monkeypatch):
    (repo / ".sakura").mkdir()
    (repo / ".sakura" / "profile.toml").write_text('protected_branches = ["main", "master", "release"]\n')
    outcome = _push_blocked(repo, monkeypatch, "release")
    assert outcome.kind == "blocked" and "protected branch" in outcome.reason


def test_PullRequest_RefusesABranchThatDoesNotMatchTheBranchPattern(repo, monkeypatch):
    outcome = _push_blocked(repo, monkeypatch, "scratch")
    assert outcome.kind == "blocked" and "branch_pattern" in outcome.reason


def test_PullRequest_RefusesABranchUnderTheIntegrationPrefixEvenWhenThePatternAllowsIt(repo, monkeypatch):
    personal = Path(__import__("os").environ["HOME"]) / ".config/sakura/profile.toml"
    personal.parent.mkdir(parents=True)
    personal.write_text('branch_pattern = "^.*$"\n')
    outcome = _push_blocked(repo, monkeypatch, "sakura-integration/7")
    assert outcome.kind == "blocked" and "sakura-integration/" in outcome.reason


def test_PullRequest_ARepositoryLayerChoosingAnotherIntegrationBranch_CannotUnprotectTheHostsOne(repo, monkeypatch):
    personal = Path(__import__("os").environ["HOME"]) / ".config/sakura/profile.toml"
    personal.parent.mkdir(parents=True)
    personal.write_text('integration_branch = "develop"\nbranch_pattern = "^.*$"\n')
    (repo / ".sakura").mkdir()
    (repo / ".sakura" / "profile.toml").write_text('integration_branch = "dev"\n')
    outcome = _push_blocked(repo, monkeypatch, "develop")
    assert outcome.kind == "blocked" and "protected branch" in outcome.reason


def test_PullRequest_RefusesToPushABranchOnlyTheHostProfileProtects(repo, monkeypatch):
    personal = Path(__import__("os").environ["HOME"]) / ".config/sakura/profile.toml"
    personal.parent.mkdir(parents=True)
    personal.write_text('protected_branches = ["main", "master", "release/stable"]\nbranch_pattern = "^.*$"\n')
    outcome = _push_blocked(repo, monkeypatch, "release/stable")
    assert outcome.kind == "blocked" and "protected branch" in outcome.reason


def test_a_failed_pull_request_lookup_says_why_in_the_blocked_reason(repo, monkeypatch):
    ctx = _context(repo, [_row()])
    failing = SimpleNamespace(exit_code=1, stopped=False, stalled=False, timed_out=False)
    log = ctx.store.operations_dir / "pull-request-lookup.log"
    monkeypatch.setattr(pull_request, "_command", lambda *_args: (
        failing, "HTTP 401: Bad credentials\nsecond line\n", log))
    outcome = pull_request._existing_draft(ctx, ctx.store.read_state(), "abc")
    assert outcome.kind == "blocked"
    assert outcome.reason == f"pull request lookup failed: HTTP 401: Bad credentials; see {log}"


def _gh_says(monkeypatch, output, exit_code=1):
    """Replace the engine's command runner: `gh` answers `output` with `exit_code`; nothing else runs."""
    def command(_ctx, _cwd, _name, _argv):
        result = SimpleNamespace(stopped=False, stalled=False, timed_out=False, exit_code=exit_code)
        return result, output, Path("pull-request-lookup.log")

    monkeypatch.setattr(pull_request, "_command", command)
    monkeypatch.setattr(pull_request, "_push", lambda *_args: None)
    monkeypatch.setattr(pull_request, "_remote_head", lambda *_args: git(_args[0], "rev-parse", "HEAD"))


@pytest.mark.parametrize("message", [
    "none of the git remotes configured for this repository point to a known GitHub host. To tell gh about a new "
    "GitHub host, please use `gh auth login`",
    "no git remotes found"])
def test_pr_remote_without_github_host_is_delivery_not_a_block(repo, monkeypatch, message):
    ctx = _context(repo, [_row()])
    ctx.store.update_state(lambda current: current.update(push_url="https://user:secret@git.example.test/team/demo.git"))
    _gh_says(monkeypatch, message)

    outcome = pull_request.run(ctx)

    assert outcome.kind == "skip"
    assert outcome.reason == "branch feat/demo pushed to https://git.example.test/team/demo.git; open the pull request yourself"
    assert "secret" not in outcome.reason
    assert ctx.store.read_state()["pr"]["head"] == git(repo, "rev-parse", "HEAD")


def test_pr_without_the_gh_command_is_delivery_not_a_block(repo, monkeypatch):
    ctx = _context(repo, [_row()])
    _gh_says(monkeypatch, "")
    monkeypatch.setattr(pull_request, "_command",
                        lambda *_args: (_ for _ in ()).throw(FileNotFoundError("gh")))

    outcome = pull_request.run(ctx)

    assert outcome.kind == "skip" and "open the pull request yourself" in outcome.reason


def test_pr_other_gh_failures_still_block(repo, monkeypatch):
    ctx = _context(repo, [_row()])
    _gh_says(monkeypatch, "HTTP 502: bad gateway")

    outcome = pull_request.run(ctx)

    assert outcome.kind == "blocked" and "pull request lookup failed" in outcome.reason


def test_pr_rerun_after_plain_remote_delivery_does_nothing(repo, monkeypatch):
    ctx = _context(repo, [_row()])
    _gh_says(monkeypatch, "no git remotes found")
    pull_request.run(ctx)
    monkeypatch.setattr(pull_request, "_push", lambda *_args: pytest.fail("pushed again"))

    assert pull_request.run(ctx).kind == "next"


def test_pr_when_every_task_needed_no_change_pushes_nothing_and_ends_as_nothing_to_change(repo, monkeypatch):
    ctx = _context(repo, [_row()])
    ctx.store.update_state(lambda current: current.update(base_commit=git(repo, "rev-parse", "HEAD")))
    monkeypatch.setattr(pull_request, "_push", lambda *_args: pytest.fail("pushed with nothing to deliver"))
    monkeypatch.setattr(pull_request, "_command", lambda *_args: pytest.fail("asked the host with nothing to deliver"))

    outcome = pull_request.run(ctx)

    assert outcome.kind == "skip" and outcome.reason.startswith("nothing to change")
    assert ctx.store.read_state()["pr"]["note"] == outcome.reason


def test_pr_with_nothing_to_change_still_needs_every_row_green(repo):
    ctx = _context(repo, [_row(False)])
    ctx.store.update_state(lambda current: current.update(base_commit=git(repo, "rev-parse", "HEAD")))

    assert pull_request.run(ctx).kind == "blocked"


def test_pr_without_a_host_tells_the_person_on_the_console_too(repo, monkeypatch, capsys):
    ctx = _context(repo, [_row()])
    _gh_says(monkeypatch, "no git remotes found")

    pull_request.run(ctx)

    assert "pushed to https://example.test/origin.git; open the pull request yourself" in capsys.readouterr().out


def _gh_runs_for_real_and_says(monkeypatch, output, exit_code=1):
    """Let `_command` run as it is, with a supervised process that writes `output` to its log and exits `exit_code`."""
    def monitored_run(_argv, log_path, **_kwargs):
        Path(log_path).write_text(output)
        return SimpleNamespace(stopped=False, stalled=False, timed_out=False, exit_code=exit_code)

    monkeypatch.setattr(pull_request.process, "monitored_run", monitored_run)
    monkeypatch.setattr(pull_request.programs, "path", lambda *_args: "/usr/bin/gh")
    monkeypatch.setattr(pull_request.programs, "environment", lambda *_args: {})
    monkeypatch.setattr(pull_request, "_push", lambda *_args: None)
    monkeypatch.setattr(pull_request, "_remote_head", lambda *_args: git(_args[0], "rev-parse", "HEAD"))


def test_pr_remote_without_github_host_does_not_print_ghs_raw_line_before_our_note(repo, monkeypatch, capsys):
    ctx = _context(repo, [_row()])
    raw = "none of the git remotes configured for this repository point to a known GitHub host."
    _gh_runs_for_real_and_says(monkeypatch, raw)

    outcome = pull_request.run(ctx)

    printed = capsys.readouterr().out
    assert outcome.kind == "skip" and "known GitHub host" not in printed
    assert printed.strip() == "branch feat/demo pushed to https://example.test/origin.git; open the pull request yourself"
    assert raw in (ctx.store.operations_dir / "pull-request-lookup.log").read_text()


def test_pr_other_gh_failures_still_print_what_gh_said(repo, monkeypatch, capsys):
    ctx = _context(repo, [_row()])
    _gh_runs_for_real_and_says(monkeypatch, "HTTP 502: bad gateway\n")

    assert pull_request.run(ctx).kind == "blocked"
    assert "HTTP 502: bad gateway" in capsys.readouterr().out


def test_Push_CarriesThePersonsCredentialSettings_ResolvedByTheEngine_AndNoOtherKey(repo, tmp_path, monkeypatch):
    # The task's own repository has no credential: the push from the engine's store gets the person's, and only those.
    recorded = _bare(tmp_path / "recorded.git")
    helper = tmp_path / "helper"
    helper.write_text("#!/bin/sh\nexit 0\n")
    helper.chmod(0o755)
    git(repo, "config", "credential.helper", str(helper))
    git(repo, "config", "sendemail.smtpPass", "smtp-password-canary")
    ctx = _context(repo, [_row()])
    git(repo, "branch", "feat/demo")
    seen = {}
    real_command = pull_request._command

    def command(ctx_, cwd, name, argv, env=None):
        seen.update(env or {})
        return real_command(ctx_, cwd, name, argv, env)

    monkeypatch.setattr(pull_request, "_command", command)
    assert pull_request._push(ctx, repo, git(repo, "rev-parse", "HEAD"), "feat/demo", str(recorded)) is None
    settings = {seen[f"GIT_CONFIG_KEY_{index}"]: seen[f"GIT_CONFIG_VALUE_{index}"]
                for index in range(int(seen["GIT_CONFIG_COUNT"]))}
    assert settings == {"credential.helper": str(helper)}
    assert "smtp-password-canary" not in " ".join(seen.values())
    assert git(recorded, "rev-parse", "refs/heads/feat/demo") == git(repo, "rev-parse", "HEAD")


def test_PullRequest_ABranchMovedAfterValidation_PushesTheValidatedHeadOnly(repo, tmp_path, monkeypatch):
    # A detached survivor moves the task branch after the receipts were checked: the push names the validated head.
    remote = _bare(tmp_path / "remote.git")
    ctx = _context(repo, [_row()])
    ctx.store.update_state(lambda state: state.update(push_url=str(remote)))
    validated = git(repo, "rev-parse", "HEAD")
    git(repo, "branch", "feat/demo")
    unverified = git(repo, "commit-tree", f"{validated}^{{tree}}", "-p", validated, "-m", "unverified")

    def survivor_moves_the_branch(*_args):
        git(repo, "update-ref", "refs/heads/feat/demo", unverified)
        return ""

    monkeypatch.setattr(pull_request, "_unpushable_branch", survivor_moves_the_branch)
    monkeypatch.setattr(pull_request, "_existing_draft", lambda *_args: pull_request.PhaseOutcome("skip"))

    outcome = pull_request.run(ctx)

    assert git(remote, "rev-parse", "refs/heads/feat/demo") == validated
    assert outcome.kind == "skip" and ctx.store.read_state()["pr"]["head"] == validated
    assert subprocess.run(["git", "-C", str(remote), "cat-file", "-e", unverified], capture_output=True).returncode != 0


def test_PullRequest_ASurvivorCommitOnTheTaskBranchBeforeDelivery_IsNeverPushed(repo, tmp_path, monkeypatch):
    # Review 11, finding 1: a survivor adds a commit with Git's plumbing and moves the branch before delivery, and
    # even receipts and a green full gate at that commit do not make it the delivered head.
    remote = _bare(tmp_path / "remote.git")
    rows = [_row()]
    ctx = _context(repo, rows)
    ctx.store.update_state(lambda state: state.update(push_url=str(remote)))
    engine = git(repo, "rev-parse", "HEAD")
    (repo / "unreviewed.txt").write_text("never reviewed\n")
    git(repo, "add", "unreviewed.txt")
    tree = git(repo, "write-tree")
    unreviewed = git(repo, "commit-tree", tree, "-p", engine, "-m", "unreviewed")
    git(repo, "update-ref", "refs/heads/main", unreviewed)
    git(repo, "reset", "-q", "--hard", unreviewed)
    _receipts_at_head(ctx, repo, rows)
    ctx.store.update_state(lambda state: state.update(full_green_head=unreviewed))
    monkeypatch.setattr(pull_request, "_existing_draft", lambda *_args: pull_request.PhaseOutcome("skip"))

    blocked = pull_request.run(ctx)

    assert blocked.kind == "blocked" and not git(remote, "for-each-ref")
    ctx.store.update_state(lambda state: state.update(full_green_head=engine))
    _receipts_at(ctx, rows, engine)

    delivered = pull_request.run(ctx)

    assert delivered.kind == "skip" and git(remote, "rev-parse", "refs/heads/feat/demo") == engine
    assert subprocess.run(["git", "-C", str(remote), "cat-file", "-e", unreviewed], capture_output=True).returncode != 0


def _receipts_at(ctx, rows, head):
    evidence = ctx.store.evidence_dir / "fixture.log"
    for row in rows:
        checks.record_receipt(ctx.store, row, contract.rows_digest(rows), head, 0, evidence, checks.EVALUATOR_RECEIPTS_KEY)


# -- delivery holds on a conflict with a lane that delivered first -------------------------------------------------


def _other_lane_delivered(repo, base, content="the other lane's change\n", name="work.txt", task="other", pr=None, at=None):
    """A second lane of the project that already delivered one commit built on `base`, sealed in the engine's store."""
    from sakura.flow import sealed

    head_here = git(repo, "rev-parse", "HEAD")
    git(repo, "checkout", "-q", "-b", f"feat/{task}", base)
    (repo / name).write_text(content)
    git(repo, "add", name)
    git(repo, "commit", "-qm", "the other lane's change")
    delivered = git(repo, "rev-parse", "HEAD")
    sealed.seal(repo, delivered)
    git(repo, "checkout", "-q", "main")
    assert git(repo, "rev-parse", "HEAD") == head_here
    record = pr(delivered) if pr else {"url": "https://example.test/pull/7", "head": delivered}
    pushed_at = {"delivery": {"kind": "push", "head": delivered, "branch": f"feat/{task}", "url": "", "at": at}} if at else {}
    make_context(repo, task=task, state={"base_commit": base, "branch": f"feat/{task}", "base_branch": "main",
                                         "status": "report_ready", "pr": record, **pushed_at})
    return delivered


def _delivery_double(monkeypatch, repo, pushed):
    monkeypatch.setattr(pull_request, "_push", lambda _ctx, _store, head, *_rest: pushed.append(head))
    monkeypatch.setattr(pull_request, "_remote_head", lambda *_args: pushed[-1])
    monkeypatch.setattr(pull_request, "_existing_draft", lambda *_args: "https://example.test/pull/42")


def test_Delivery_ConflictWithDeliveredLane_WaitsWithFiles(repo, monkeypatch):
    ctx = _context(repo, [_row()])
    base = ctx.store.read_state()["base_commit"]
    _other_lane_delivered(repo, base)
    pushed = []
    _delivery_double(monkeypatch, repo, pushed)

    outcome = pull_request.run(ctx)

    assert outcome.kind == "await" and outcome.gate == "overlap" and pushed == []
    assert "work.txt" in outcome.reason and f"{repo.name}/other" in outcome.reason
    pending = ctx.store.read_state()["overlap"]
    assert pending["lane"] == f"{repo.name}/other" and pending["files"] == ["work.txt"]
    assert pending["head"] == git(repo, "rev-parse", "HEAD")


def test_Delivery_SameFilesNoConflict_Delivers(repo, monkeypatch):
    ctx = _context(repo, [_row()])
    base = ctx.store.read_state()["base_commit"]
    _other_lane_delivered(repo, base, content="the task's change\n")
    pushed = []
    _delivery_double(monkeypatch, repo, pushed)

    assert pull_request.run(ctx).kind == "next" and len(pushed) == 1


def test_Delivery_OtherLaneWithoutADelivery_Delivers(repo, monkeypatch):
    ctx = _context(repo, [_row()])
    make_context(repo, task="other", state={"base_commit": ctx.store.read_state()["base_commit"], "pr": {}})
    pushed = []
    _delivery_double(monkeypatch, repo, pushed)
    assert pull_request.run(ctx).kind == "next" and len(pushed) == 1


def test_Delivery_OtherLaneDeliveredBeforeOurBase_Delivers(repo, monkeypatch):
    """A head already in this lane's base is no news: its work is part of ours."""
    old = git(repo, "rev-parse", "HEAD")
    ctx = _context(repo, [_row()])
    make_context(repo, task="other", state={"base_commit": old, "pr": {"url": "https://example.test/pull/1", "head": old}})
    pushed = []
    _delivery_double(monkeypatch, repo, pushed)
    assert pull_request.run(ctx).kind == "next"


def test_Delivery_ApproveOverlap_PushesEngineHead(repo, monkeypatch):
    from sakura.flow import cli

    ctx = _context(repo, [_row()])
    _other_lane_delivered(repo, ctx.store.read_state()["base_commit"])
    pushed = []
    _delivery_double(monkeypatch, repo, pushed)
    assert pull_request.run(ctx).kind == "await"
    cli._require_approvable(ctx.store, "demo", "overlap")
    cli._record_approval(ctx.store, "overlap", "cli:person")

    outcome = pull_request.run(ctx)

    assert outcome.kind == "next" and pushed == [git(repo, "rev-parse", "HEAD")]
    assert ctx.store.read_state()["approvals"]["overlap"]["head"] == pushed[0]


def test_Delivery_ApprovalOfAnEarlierHead_DoesNotCoverTheNewOne(repo, monkeypatch):
    ctx = _context(repo, [_row()])
    _other_lane_delivered(repo, ctx.store.read_state()["base_commit"])
    _delivery_double(monkeypatch, repo, [])
    ctx.store.update_state(lambda state: state["approvals"].update(overlap={"at": "t", "head": "0" * 40}))
    assert pull_request.run(ctx).kind == "await"


def test_Delivery_ApprovalOfOneConflict_DoesNotCoverALaterConflictWithAnotherLane(repo, monkeypatch):
    from sakura.flow import cli

    ctx = _context(repo, [_row()])
    base = ctx.store.read_state()["base_commit"]
    _other_lane_delivered(repo, base)
    pushed = []
    _delivery_double(monkeypatch, repo, pushed)
    assert pull_request.run(ctx).kind == "await"
    first = ctx.store.read_state()["overlap"]
    cli._record_approval(ctx.store, "overlap", "cli:person")
    third = _other_lane_delivered(repo, base, content="a third lane's change\n", task="third")

    outcome = pull_request.run(ctx)

    assert outcome.kind == "await" and outcome.gate == "overlap" and pushed == []
    again = ctx.store.read_state()["overlap"]
    assert again["revision"] != first["revision"] and {item["head"] for item in again["conflicts"]} >= {third}
    assert len(again["conflicts"]) == 2 and f"{repo.name}/third" in outcome.reason + " ".join(i["lane"] for i in again["conflicts"])
    cli._record_approval(ctx.store, "overlap", "cli:person")
    assert pull_request.run(ctx).kind == "next" and len(pushed) == 1


def test_Delivery_OverlapApprovalWithNoRevision_DoesNotCoverTheConflict(repo, monkeypatch):
    ctx = _context(repo, [_row()])
    _other_lane_delivered(repo, ctx.store.read_state()["base_commit"])
    _delivery_double(monkeypatch, repo, [])
    head = git(repo, "rev-parse", "HEAD")
    ctx.store.update_state(lambda state: state["approvals"].update(overlap={"at": "t", "head": head}))
    assert pull_request.run(ctx).kind == "await"


def test_Waits_OverlapRevision_ChangesWhenAnotherLaneDeliversAConflict(repo, monkeypatch):
    from sakura.flow import waits

    ctx = _context(repo, [_row()])
    base = ctx.store.read_state()["base_commit"]
    _other_lane_delivered(repo, base)
    _delivery_double(monkeypatch, repo, [])
    pull_request.run(ctx)
    before = waits.approval_revision(ctx.store, ctx.store.read_state(), "overlap")
    _other_lane_delivered(repo, base, content="a third lane's change\n", task="third")
    pull_request.run(ctx)
    assert waits.approval_revision(ctx.store, ctx.store.read_state(), "overlap") != before


def test_Waits_OverlapWithTwoLanes_NamesEveryLaneAndFileTheApprovalBindsButNoHead(repo, monkeypatch):
    from sakura.flow import cli, tasks, waits

    ctx = _context(repo, [_row()], names=("x.py", "y.py"))
    base = ctx.store.read_state()["base_commit"]
    b = _other_lane_delivered(repo, base, content="B's change\n", name="x.py", task="laneb")
    _delivery_double(monkeypatch, repo, [])
    assert pull_request.run(ctx).kind == "await"
    drawn = waits.approval_revision(ctx.store, ctx.store.read_state(), "overlap")
    c = _other_lane_delivered(repo, base, content="C's change\n", name="y.py", task="lanec")
    assert pull_request.run(ctx).kind == "await"
    ctx.store.update_state(lambda state: state.update(
        status="awaiting_approval", reason="awaiting approval: sakura flow approve demo overlap (x)"))
    [wait] = [wait for wait in waits.collect([repo]) if wait.task == "demo"]
    for lane, head, name in ((f"{repo.name}/laneb", b, "x.py"), (f"{repo.name}/lanec", c, "y.py")):
        assert lane in wait.detail and name in wait.detail and head[:12] not in wait.detail
    current = waits.approval_revision(ctx.store, ctx.store.read_state(), "overlap")
    assert current != drawn
    with pytest.raises(tasks.TaskStartError, match="no longer waiting"):
        cli._require_revision(drawn, current, "demo")


def test_Approve_OverlapWhenNothingWaits_Refused(repo):
    from sakura.flow import cli, tasks

    ctx = _context(repo, [_row()])
    with pytest.raises(tasks.TaskStartError, match="no overlap waits"):
        cli._require_approvable(ctx.store, "demo", "overlap")
    assert cli._gate_name("overlap") == "overlap"


def test_Waits_OverlapGate_NamesTheOtherLaneAndTheFiles(repo, monkeypatch):
    from sakura.flow import waits

    ctx = _context(repo, [_row()])
    _other_lane_delivered(repo, ctx.store.read_state()["base_commit"])
    _delivery_double(monkeypatch, repo, [])
    pull_request.run(ctx)
    ctx.store.update_state(lambda state: state.update(
        status="awaiting_approval", reason="awaiting approval: sakura flow approve demo overlap (x)"))
    found = [wait for wait in waits.collect([repo]) if wait.task == "demo"]
    assert [(wait.kind, wait.title) for wait in found] == [("approval", "Delivery overlaps another lane")]
    assert f"{repo.name}/other" in found[0].detail and "work.txt" in found[0].detail
    revision = waits.approval_revision(ctx.store, ctx.store.read_state(), "overlap")
    assert found[0].id.endswith(f'"approval","{waits.bound_key("overlap", revision)}"]')


# -- the overlap check compares against every kind of delivery (proof defect D11) ----------------------------------------


PUSHED_NOTE = "branch feat/other pushed to /srv/remote.git; open the pull request yourself"
ADJACENT_BASE = "a\nb\nc\nd\n"


def _adjacent_lines_context(repo):
    """The base holds f.txt; this lane edits its second line, the other lane edits the third (adjacent lines conflict)."""
    (repo / "f.txt").write_text(ADJACENT_BASE)
    git(repo, "add", "f.txt")
    git(repo, "commit", "-qm", "add f.txt")
    return _context(repo, [_row()], names=(), contents={"f.txt": "a\nB\nc\nd\n"})


@pytest.mark.parametrize("record", [
    lambda head: {"url": "", "head": head, "note": PUSHED_NOTE},
    lambda head: {"url": "https://example.test/pull/7", "head": head},
], ids=["push without a pull request", "pull request"])
def test_Delivery_OtherLaneDeliveredAdjacentLinesWithoutAPullRequest_TheSecondWaitsWithTheFiles(repo, monkeypatch, record):
    ctx = _adjacent_lines_context(repo)
    base = ctx.store.read_state()["base_commit"]
    _other_lane_delivered(repo, base, content="a\nb\nC\nd\n", name="f.txt", pr=record)
    pushed = []
    _delivery_double(monkeypatch, repo, pushed)

    outcome = pull_request.run(ctx)

    assert outcome.kind == "await" and outcome.gate == "overlap" and pushed == []
    assert "f.txt" in outcome.reason and f"{repo.name}/other" in outcome.reason


def test_Delivery_PushWithoutAPullRequest_RecordsTheDeliveryInOnePlace(repo, tmp_path, monkeypatch):
    from sakura.flow import delivery

    remote = _bare(tmp_path / "remote.git")
    ctx = _context(repo, [_row()])
    ctx.store.update_state(lambda state: state.update(push_url=str(remote)))
    monkeypatch.setattr(pull_request, "_existing_draft", lambda *_args: pull_request.PhaseOutcome("skip"))
    assert pull_request.run(ctx).kind == "skip"
    state = ctx.store.read_state()
    found = delivery.of(state)
    assert found == {"kind": "local", "head": git(repo, "rev-parse", "HEAD"), "branch": "feat/demo", "url": ""}
    assert state["pr"]["head"] == found["head"] and "pushed to" in state["pr"]["note"]


def test_Delivery_PullRequest_RecordsTheDeliveryWithItsUrl(repo, monkeypatch):
    from sakura.flow import delivery

    ctx = _context(repo, [_row()])
    _delivery_double(monkeypatch, repo, [])
    assert pull_request.run(ctx).kind == "next"
    found = delivery.of(ctx.store.read_state())
    assert found["kind"] == "pull_request" and found["url"] == "https://example.test/pull/42" and found["branch"] == "feat/demo"


# -- review 25, findings 6 and 7: the verified push is recorded at once, and deliveries of a project take turns -------------------


def test_Delivery_AVerifiedPushWhoseLookupBlocks_IsAlreadyRecordedAsDelivered(repo, monkeypatch):
    from sakura.flow import delivery

    ctx = _context(repo, [_row()])
    pushed = []
    _delivery_double(monkeypatch, repo, pushed)
    monkeypatch.setattr(pull_request, "_existing_draft",
                        lambda *_args: pull_request.PhaseOutcome("blocked", "pull request lookup failed: timed out"))

    outcome = pull_request.run(ctx)

    head = git(repo, "rev-parse", "HEAD")
    assert outcome.kind == "blocked" and pushed == [head]
    found = delivery.of(ctx.store.read_state())
    assert found["head"] == head and found["kind"] in ("push", "local") and found["url"] == ""


def test_Delivery_ABlockedLookup_MakesAnotherLaneWaitOnTheConflict(repo, monkeypatch):
    base = git(repo, "rev-parse", "HEAD")
    one = _context(repo, [_row()], contents={"work.txt": "lane one\n"}, task="one", branch="feat/one")
    pushed = []
    _delivery_double(monkeypatch, repo, pushed)
    monkeypatch.setattr(pull_request, "_existing_draft",
                        lambda *_args: pull_request.PhaseOutcome("blocked", "pull request lookup failed: timed out"))
    assert pull_request.run(one).kind == "blocked"
    git(repo, "checkout", "-q", "-b", "two-line", base)
    two = _context(repo, [_row()], contents={"work.txt": "lane two\n"}, task="two", branch="feat/two")
    monkeypatch.setattr(pull_request, "_existing_draft", lambda *_args: "https://example.test/pull/2")

    outcome = pull_request.run(two)

    assert (outcome.kind, outcome.gate) == ("await", "overlap") and pushed == [pushed[0]]


def test_Delivery_ALookupThatFindsAPullRequestLater_UpgradesTheRecordWithItsUrl(repo, monkeypatch):
    from sakura.flow import delivery

    ctx = _context(repo, [_row()])
    _delivery_double(monkeypatch, repo, [])
    monkeypatch.setattr(pull_request, "_existing_draft", lambda *_args: pull_request.PhaseOutcome("blocked", "timed out"))
    assert pull_request.run(ctx).kind == "blocked"
    monkeypatch.setattr(pull_request, "_existing_draft", lambda *_args: "https://example.test/pull/42")

    assert pull_request.run(ctx).kind == "next"
    assert delivery.of(ctx.store.read_state())["url"] == "https://example.test/pull/42"


def test_Delivery_TwoConflictingLanesDeliveringTogether_OnlyOneDeliversTheOtherWaits(repo, monkeypatch):
    import threading

    from sakura.flow import final_head

    base = git(repo, "rev-parse", "HEAD")
    one = _context(repo, [_row()], contents={"work.txt": "lane one\n"}, task="one", branch="feat/one")
    git(repo, "checkout", "-q", "-b", "two-line", base)
    two = _context(repo, [_row()], contents={"work.txt": "lane two\n"}, task="two", branch="feat/two")
    heads = {"feat/one": final_head.proved(one)[1], "feat/two": final_head.proved(two)[1]}
    inside, release, pushes, results = threading.Event(), threading.Event(), [], {}

    def slow_push(_ctx, _store, head, branch, _url):
        pushes.append(branch)
        inside.set()
        release.wait(10)

    monkeypatch.setattr(pull_request, "_push", slow_push)
    monkeypatch.setattr(pull_request, "_remote_head", lambda _repo, branch, _url: heads[branch])
    monkeypatch.setattr(pull_request, "_existing_draft", lambda *_args: "https://example.test/pull/1")
    runners = {name: threading.Thread(target=lambda name=name, ctx=ctx: results.update({name: pull_request.run(ctx)}))
               for name, ctx in (("one", one), ("two", two))}

    runners["one"].start()
    assert inside.wait(10)
    runners["two"].start()
    runners["two"].join(1)
    assert runners["two"].is_alive(), "the second lane waits for the project's delivery lock while the first pushes"
    release.set()
    for runner in runners.values():
        runner.join(20)

    assert results["one"].kind == "next"
    assert (results["two"].kind, results["two"].gate) == ("await", "overlap")
    assert pushes == ["feat/one"]


def test_pr_body_with_the_checks_switched_off_says_so_and_lists_no_provers():
    state = {"preset": "lean", "base_branch": "main"}
    body = pull_request.pr_body(state, [_row(False)], config.PresetSteps(False, False, False), [], "report.md",
                                checks_on=False)
    assert "Verification: checks were switched off by the preset, so no contract row was run" in body
    assert "Provers" not in body and "not verified by an evaluator" not in body


# -- the order of the pushes is the real one (N20)

FIRST_PUSH, SECOND_PUSH = "2026-11-19T21:35:00+00:00", "2026-11-19T22:03:00+00:00"


def _this_lane_pushed_at(ctx, moment):
    head = ctx.store.read_state()["branch"] and git(ctx.repo, "rev-parse", "HEAD")
    ctx.store.update_state(lambda state: state.update(delivery={"kind": "push", "head": head, "branch": "feat/demo",
                                                                "url": "", "at": moment}))


def test_Delivery_ALaneThatPushedFirstAndResumesItsPullRequestStep_IsNotHeldByALaneThatPushedAfterIt(repo, monkeypatch):
    ctx = _context(repo, [_row()])
    _other_lane_delivered(repo, ctx.store.read_state()["base_commit"], at=SECOND_PUSH)
    _this_lane_pushed_at(ctx, FIRST_PUSH)
    pushed = []
    _delivery_double(monkeypatch, repo, pushed)

    outcome = pull_request.run(ctx)

    assert outcome.kind == "next", outcome.reason


def test_Delivery_ALaneThatPushedSecond_IsHeldByTheLaneThatPushedBeforeIt(repo, monkeypatch):
    ctx = _context(repo, [_row()])
    _other_lane_delivered(repo, ctx.store.read_state()["base_commit"], at=FIRST_PUSH)
    _this_lane_pushed_at(ctx, SECOND_PUSH)
    _delivery_double(monkeypatch, repo, [])

    outcome = pull_request.run(ctx)

    assert (outcome.kind, outcome.gate) == ("await", "overlap") and f"{repo.name}/other" in outcome.reason


def test_Delivery_APushIsRecordedWithItsMoment_AndAPullRequestFoundLaterKeepsIt(repo):
    from sakura.flow import delivery

    ctx = _context(repo, [_row()])

    delivery.record(ctx.store, "push", "a" * 40, "feat/demo")
    first = ctx.store.read_state()["delivery"]["at"]
    delivery.record(ctx.store, "pull_request", "a" * 40, "feat/demo", url="https://example.test/pull/1")
    kept = ctx.store.read_state()["delivery"]["at"]
    delivery.record(ctx.store, "push", "b" * 40, "feat/demo")

    assert first and kept == first and ctx.store.read_state()["delivery"]["at"] >= first
    assert delivery.delivered_at(ctx.store.read_state()) == ctx.store.read_state()["delivery"]["at"]
    assert "at" not in delivery.of(ctx.store.read_state()), "the shape other readers get is unchanged"


def test_Delivery_ALaneThatPushesANewHeadAfterAnotherLanePushed_IsHeldEvenThoughItPushedAnEarlierHeadFirst(repo, monkeypatch):
    ctx = _context(repo, [_row()])
    _other_lane_delivered(repo, ctx.store.read_state()["base_commit"], at=SECOND_PUSH)
    ctx.store.update_state(lambda state: state.update(delivery={"kind": "push", "head": "0" * 40, "branch": "feat/demo",
                                                                "url": "", "at": FIRST_PUSH}))
    _delivery_double(monkeypatch, repo, [])

    outcome = pull_request.run(ctx)

    assert (outcome.kind, outcome.gate) == ("await", "overlap"), "the earlier push was of another head"


# -- resuming a delivered lane to push a late commit looks only at what the late commit touched (N34)

BEFORE_PUSH = "2026-11-19T20:00:00+00:00"


def _delivered_lane_with_a_late_commit(repo):
    """This lane pushed its head at FIRST_PUSH; a message to its builder then left one more commit, `late.txt`, now the head."""
    ctx = _context(repo, [_row()])
    base = ctx.store.read_state()["base_commit"]
    first = git(repo, "rev-parse", "HEAD")
    ctx.store.update_state(lambda state: state.update(delivery={"kind": "push", "head": first, "branch": "feat/demo",
                                                                "url": "", "at": FIRST_PUSH}))
    (repo / "late.txt").write_text("the late commit\n")
    git(repo, "add", "late.txt")
    git(repo, "commit", "-qm", "the late commit")
    engine_head(ctx)
    ctx.store.update_state(lambda state: state.update(full_green_head=git(repo, "rev-parse", "HEAD")))
    ctx.store.update_state(lambda state: state.update(full_green_inputs_sha256=integrate.full_tests_inputs_hash(ctx)))
    _receipts_at_head(ctx, repo, [_row()])
    return ctx, base


def test_Delivery_ALateCommitPushedAgain_IsNotHeldByLanesThatChangedOtherFilesThanTheLateCommit(repo, monkeypatch):
    ctx, base = _delivered_lane_with_a_late_commit(repo)
    _other_lane_delivered(repo, base, name="work.txt", task="other", at=SECOND_PUSH)  # conflicts with the first push's work.txt
    _delivery_double(monkeypatch, repo, [])

    outcome = pull_request.run(ctx)

    assert outcome.kind == "next", outcome.reason


def test_Delivery_ALateCommitPushedAgain_IsHeldOnlyByALaneThatDeliveredAfterTheFirstPushAndChangedTheLateFile(repo, monkeypatch):
    ctx, base = _delivered_lane_with_a_late_commit(repo)
    _other_lane_delivered(repo, base, name="late.txt", task="after", at=SECOND_PUSH)
    _other_lane_delivered(repo, base, name="work.txt", task="elsewhere", at=SECOND_PUSH)
    _delivery_double(monkeypatch, repo, [])

    outcome = pull_request.run(ctx)

    assert (outcome.kind, outcome.gate) == ("await", "overlap")
    conflicts = ctx.store.read_state()["overlap"]["conflicts"]
    assert [(item["lane"], item["files"]) for item in conflicts] == [(f"{repo.name}/after", ["late.txt"])]


def test_Delivery_ALateCommitPushedAgain_IsNotHeldByALaneThatDeliveredBeforeTheFirstPush(repo, monkeypatch):
    ctx, base = _delivered_lane_with_a_late_commit(repo)
    _other_lane_delivered(repo, base, name="late.txt", task="before", at=BEFORE_PUSH)
    _delivery_double(monkeypatch, repo, [])

    outcome = pull_request.run(ctx)

    assert outcome.kind == "next", outcome.reason


def test_Delivery_ALateCommitPushedAgain_IsHeldByALaneWhoseDeliveryHasNoMoment(repo, monkeypatch):
    ctx, base = _delivered_lane_with_a_late_commit(repo)
    _other_lane_delivered(repo, base, name="late.txt", task="old")  # a delivery from before moments were kept
    _delivery_double(monkeypatch, repo, [])

    outcome = pull_request.run(ctx)

    assert (outcome.kind, outcome.gate) == ("await", "overlap"), "an order nobody can tell is not an order to skip on"


def test_Delivery_ALateFileWithASpaceInItsName_StillHoldsTheDeliveryItConflictsIn(repo, monkeypatch):
    ctx, base = _delivered_lane_with_a_late_commit(repo)
    (repo / "late file.txt").write_text("the late commit\n")
    git(repo, "add", "late file.txt")
    git(repo, "commit", "-qm", "a file with a space")
    engine_head(ctx)
    ctx.store.update_state(lambda state: state.update(full_green_head=git(repo, "rev-parse", "HEAD")))
    ctx.store.update_state(lambda state: state.update(full_green_inputs_sha256=integrate.full_tests_inputs_hash(ctx)))
    _receipts_at_head(ctx, repo, [_row()])
    _other_lane_delivered(repo, base, name="late file.txt", task="after", at=SECOND_PUSH)
    _delivery_double(monkeypatch, repo, [])

    outcome = pull_request.run(ctx)

    assert (outcome.kind, outcome.gate) == ("await", "overlap"), "the hold does not fail open for a name with a space"
    assert ctx.store.read_state()["overlap"]["conflicts"][0]["files"] == [], "such a name is counted and not shown (`changes.plain`)"
