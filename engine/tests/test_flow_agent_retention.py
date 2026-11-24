"""A message queued for a running reviewer keeps the reviewer's copy of the code until it is delivered."""

import os

import pytest

from flow_run_support import LIVE_IDENTITY
from flow_support import make_context
from sakura.flow import agent_messages, agent_resume, budget_guard, final_head, run_records, task_repository
from test_flow_agent_resume import FakeContext

CODEX_SESSION = "019a1b2c-3d4e-7f80-9a1b-2c3d4e5f6a7b"


@pytest.fixture
def ctx(repo, monkeypatch):
    monkeypatch.setattr(budget_guard, "require", lambda *args, **kwargs: None)
    return make_context(repo, own_repository=True, overrides={"roles": {"code_reviewer": "codex"}})


def base(ctx):
    return ctx.store.read_state()["base_commit"]


def begin_review(ctx, worktree, state="running"):
    run_records.begin(ctx.store, "review-1", "code_reviewer", "review", "codex", "", "low", "local", worktree, {})
    run_records.mark_running(ctx.store, "review-1", LIVE_IDENTITY)
    run_records.log_path(ctx.store, "review-1").parent.mkdir(parents=True, exist_ok=True)
    run_records.log_path(ctx.store, "review-1").write_text(f'{{"type":"thread.started","thread_id":"{CODEX_SESSION}"}}\n')
    if state == "finished":
        run_records.finish(ctx.store, "review-1", 0)


def test_ReviewCopy_NothingIsHeld_IsRemovedWhenTheReviewEnds(ctx):
    with final_head.review_copy(ctx, "review-x", base(ctx), base(ctx)) as (repository, worktree):
        begin_review(ctx, worktree)

    assert not worktree.exists() and not repository.exists()


def test_ReviewCopy_AMessageIsHeldForTheRunningReview_KeepsTheCopyAndAWorkerDeliversThenRemovesIt(ctx):
    requests = []
    with final_head.review_copy(ctx, "review-x", base(ctx), base(ctx)) as (repository, worktree):
        begin_review(ctx, worktree)
        agent_messages.hold(ctx.store, "review-1", "explain finding 2", "cli:person")
        run_records.finish(ctx.store, "review-1", 0)

    assert worktree.is_dir() and repository.is_dir(), "the queued message still needs the reviewer's folder"

    agent_resume.work(FakeContext(ctx.store, requests), "review-1")

    assert [request.template for request in requests] == ["explain finding 2"]
    assert os.path.realpath(requests[0].repository) == os.path.realpath(repository)
    assert not worktree.exists() and not repository.exists(), "removed once nothing is left to deliver"
    state = ctx.store.read_state()
    assert not state.get(task_repository.RETAINED_KEY) and not state.get(task_repository.ROLE_REPOSITORIES_KEY)


def test_ReviewCopy_AContinuationAlreadyStarting_KeepsTheCopyToo(ctx):
    with final_head.review_copy(ctx, "review-x", base(ctx), base(ctx)) as (repository, worktree):
        begin_review(ctx, worktree)
        run_records.finish(ctx.store, "review-1", 0)
        run_records.begin(ctx.store, "review-1.2", "code_reviewer", "review", "codex", "", "low", "local", worktree, {},
                          resumes="review-1", pending_text="go on")

    assert worktree.is_dir()


def test_Cleanup_StillRemovesARetainedCopyAtTheEndOfTheLane(ctx):
    with final_head.review_copy(ctx, "review-x", base(ctx), base(ctx)) as (repository, worktree):
        begin_review(ctx, worktree)
        agent_messages.hold(ctx.store, "review-1", "never delivered", "cli:person")

    task_repository.remove_role_repository(ctx.store, repository)

    assert not worktree.exists() and not ctx.store.read_state().get(task_repository.RETAINED_KEY)
