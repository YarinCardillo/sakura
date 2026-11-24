"""Proposals: an agent suggests a change to a knowledge folder; it is written only when the person accepts."""

import hashlib
import json
import os
from datetime import datetime, timezone

import pytest

from flow_support import make_context
from sakura import engine_home, install, knowledge, proposal_cli, proposals
from sakura.flow import board, brakes, cage, exits, waits
from test_flow_cage import _session

NOW = datetime(2026, 11, 16, 12, 0, tzinfo=timezone.utc)
AUTHOR = board.Author("role", "demo", "builder", "build-1")
PERSON = "overlay:owner@example.com:nPHONE1"


@pytest.fixture
def notes(tmp_path):
    folder = tmp_path / "notes"
    folder.mkdir()
    (folder / "style.md").write_text("Use plain words.\n")
    return folder


@pytest.fixture
def folders(notes):
    return {"shared": notes.resolve()}


def _propose(repo, folders, path="style.md", text="Use short words.\n", reason="Plain wording helps.", folder="shared",
             author=AUTHOR):
    return proposals.propose(engine_home.root(), folders, repo, folder, path, text, reason, author, NOW)


def _accept(folders, made, by=PERSON):
    return proposals.accept(engine_home.root(), made.id, made.digest, folders, by, NOW)


def _pending_files():
    return sorted((engine_home.root() / proposals.FOLDER).glob("p*.json"))


def test_Proposal_Propose_RecordsTheTargetAsItWasAndWritesNothing(repo, folders, notes):
    made = _propose(repo, folders)
    stored = json.loads(_pending_files()[0].read_text())
    assert stored["id"] == made.id and stored["folder"] == "shared" and stored["path"] == "style.md"
    assert stored["text"] == "Use short words.\n" and stored["reason"] == "Plain wording helps."
    assert stored["base_digest"] == hashlib.sha256(b"Use plain words.\n").hexdigest()
    assert stored["folder_path"] == str(notes.resolve()) and stored["repo"] == str(repo)
    assert (stored["task"], stored["role"], stored["run"]) == ("demo", "builder", "build-1")
    assert (notes / "style.md").read_text() == "Use plain words.\n"
    assert oct(_pending_files()[0].stat().st_mode & 0o777) == "0o600"


def test_Proposal_NewFile_BaseDigestEmpty(repo, folders):
    assert _propose(repo, folders, path="notes/new.md").base_digest == ""


@pytest.mark.parametrize("path", ["../x.md", "a/../b.md", "/etc/x.md", "x.txt", "x", ".hidden.md", "a/.git/x.md", "a//b.md",
                                  "a b.md", "x\x00.md", "é.md", "a/b/c/d/e/f/g/h.md", "x" * 100 + ".md", "", "dir/",
                                  "sub/../../x.md", ".md"])
def test_Proposal_PathTraversal_Refused(repo, folders, path):
    with pytest.raises(proposals.ProposalRefused, match="path"):
        _propose(repo, folders, path=path)
    assert _pending_files() == []


def test_Proposal_LinkOnTheWay_Refused(repo, folders, notes, tmp_path):
    outside = tmp_path / "outside"
    outside.mkdir()
    (notes / "alias").symlink_to(outside)
    (notes / "ln.md").symlink_to(notes / "style.md")
    for path in ("alias/x.md", "ln.md"):
        with pytest.raises(proposals.ProposalRefused, match="plain file"):
            _propose(repo, folders, path=path)


def test_Proposal_FolderNotConfigured_Refused(repo, folders):
    with pytest.raises(proposals.ProposalRefused, match="project knowledge folder"):
        _propose(repo, folders, folder="project")
    with pytest.raises(proposals.ProposalRefused, match="folder"):
        _propose(repo, folders, folder="../x")


@pytest.mark.parametrize("text", ["", "   \n", "x" * (proposals.MAX_TEXT + 1), "a\x1bb", "a‮b", "a​b", "ab"])
def test_Proposal_BadText_Refused(repo, folders, text):
    with pytest.raises(proposals.ProposalRefused, match="text"):
        _propose(repo, folders, text=text)


def test_Proposal_TextOfManyBytes_Refused(repo, folders):
    with pytest.raises(proposals.ProposalRefused, match="text"):
        _propose(repo, folders, text="é" * (proposals.MAX_TEXT // 2 + 1))


@pytest.mark.parametrize("reason", ["", "two\nlines", "x" * (proposals.MAX_REASON + 1), "a‮b"])
def test_Proposal_BadReason_Refused(repo, folders, reason):
    with pytest.raises(proposals.ProposalRefused, match="reason"):
        _propose(repo, folders, reason=reason)


@pytest.mark.parametrize("author", [board.Author("person"), board.Author("role", "bad name"), board.Author("engine")])
def test_Proposal_AuthorNotARoleOrMaster_Refused(repo, folders, author):
    with pytest.raises(proposals.ProposalRefused, match="author"):
        _propose(repo, folders, author=author)


def test_Proposal_TooManyWaiting_Refused(repo, folders, monkeypatch):
    monkeypatch.setattr(proposals, "MAX_PENDING", 2)
    _propose(repo, folders)
    _propose(repo, folders, path="a.md")
    with pytest.raises(proposals.ProposalRefused, match="already waiting"):
        _propose(repo, folders, path="b.md")


def test_Proposal_Accept_WritesExactText(repo, folders, notes):
    text = "Use short words.\nTwo\ttabs and a café.\n"
    made = _propose(repo, folders, text=text)
    _accept(folders, made)
    assert (notes / "style.md").read_bytes() == text.encode()
    assert [path.name for path in notes.iterdir()] == ["style.md"]


def test_Proposal_AcceptNewFileInNewFolder_Creates(repo, folders, notes):
    made = _propose(repo, folders, path="topics/words.md", text="New.\n")
    _accept(folders, made)
    assert (notes / "topics" / "words.md").read_text() == "New.\n"


def test_Proposal_Accept_ProposalNoLongerWaitingAndDecisionKept(repo, folders):
    made = _propose(repo, folders)
    _accept(folders, made)
    assert proposals.pending(engine_home.root()) == [] and _pending_files() == []
    [record] = (engine_home.root() / proposals.FOLDER / proposals.DECIDED).glob("p*.json")
    assert json.loads(record.read_text())["decision"] == {"outcome": "accepted", "by": PERSON, "at": "2026-11-16T12:00:00+00:00"}


def test_Proposal_AcceptTwice_SecondNoLongerWaiting(repo, folders):
    made = _propose(repo, folders)
    _accept(folders, made)
    with pytest.raises(proposals.ProposalRefused, match="no longer waiting"):
        _accept(folders, made)


def test_Proposal_AcceptAfterTargetChanged_Refused(repo, folders, notes):
    made = _propose(repo, folders)
    (notes / "style.md").write_text("The person edited it meanwhile.\n")
    with pytest.raises(proposals.ProposalRefused, match="changed since"):
        _accept(folders, made)
    assert (notes / "style.md").read_text() == "The person edited it meanwhile.\n"
    assert len(proposals.pending(engine_home.root())) == 1


def test_Proposal_AcceptNewFileThatAppearedMeanwhile_Refused(repo, folders, notes):
    made = _propose(repo, folders, path="new.md", text="Mine.\n")
    (notes / "new.md").write_text("Theirs.\n")
    with pytest.raises(proposals.ProposalRefused, match="changed since"):
        _accept(folders, made)


def test_Proposal_AcceptWhenTargetAlreadyHoldsTheText_CompletesWithoutWriting(repo, folders, notes):
    made = _propose(repo, folders)
    (notes / "style.md").write_text(made.text)
    before = (notes / "style.md").stat().st_mtime_ns
    _accept(folders, made)
    assert (notes / "style.md").stat().st_mtime_ns == before and proposals.pending(engine_home.root()) == []


def test_Proposal_AcceptWhenALinkAppearedOnTheWay_Refused(repo, folders, notes, tmp_path):
    made = _propose(repo, folders, path="topics/words.md", text="New.\n")
    outside = tmp_path / "outside"
    outside.mkdir()
    (notes / "topics").symlink_to(outside)
    with pytest.raises(proposals.ProposalRefused, match="plain file"):
        _accept(folders, made)
    assert list(outside.iterdir()) == []


def test_Proposal_AcceptWhenTheFolderMovedSinceTheProposal_Refused(repo, folders, tmp_path):
    made = _propose(repo, folders)
    other = tmp_path / "other"
    other.mkdir()
    with pytest.raises(proposals.ProposalRefused, match="folder changed"):
        _accept({"shared": other.resolve()}, made)
    with pytest.raises(proposals.ProposalRefused, match="folder changed"):
        _accept({}, made)


def test_Proposal_Reject_WritesNothingAndKeepsTheDecision(repo, folders, notes):
    made = _propose(repo, folders)
    proposals.reject(engine_home.root(), made.id, made.digest, PERSON, NOW)
    assert (notes / "style.md").read_text() == "Use plain words.\n" and proposals.pending(engine_home.root()) == []
    [record] = (engine_home.root() / proposals.FOLDER / proposals.DECIDED).glob("p*.json")
    assert json.loads(record.read_text())["decision"]["outcome"] == "rejected"
    with pytest.raises(proposals.ProposalRefused, match="no longer waiting"):
        proposals.reject(engine_home.root(), made.id, made.digest, PERSON, NOW)


@pytest.mark.parametrize("identifier", ["", "../x", "p1", "pzzzzzzzz", "p0123456789", "P01234567", "p01234567/../x"])
def test_Proposal_BadId_Refused(identifier):
    with pytest.raises(proposals.ProposalRefused, match="proposal id"):
        proposals.reject(engine_home.root(), identifier, "0" * 12, PERSON, NOW)


def test_Proposal_Pending_OnlyOfTheProjectAskedFor(repo, folders, tmp_path):
    _propose(repo, folders)
    assert len(proposals.pending(engine_home.root(), repo)) == 1
    assert proposals.pending(engine_home.root(), tmp_path / "other-project") == []


def test_Proposal_PendingSkipsFilesThatAreNotProposals(repo, folders):
    _propose(repo, folders)
    (engine_home.root() / proposals.FOLDER / "p00000000.json").write_text("{not json")
    (engine_home.root() / proposals.FOLDER / "notes.txt").write_text("x")
    assert len(proposals.pending(engine_home.root())) == 1


# -- the command: the person only -------------------------------------------------------------------------------------


@pytest.fixture
def configured(repo, notes, monkeypatch):
    monkeypatch.setattr(cage, "writable_roots", lambda: ())
    (engine_home.root() / "config.toml").write_text(f'[knowledge]\nshared = "{notes}"\n')
    monkeypatch.chdir(repo)
    install.install(repo)


def _proposed(repo, notes):
    return _propose(repo, {"shared": notes.resolve()})


def test_ProposalCli_Accept_WritesAndSaysSo(repo, notes, configured, capsys, monkeypatch):
    monkeypatch.setenv("SAKURA_ACTOR_TRUSTED", "1")  # the cockpit's own child carries the viewer's identity
    made = _proposed(repo, notes)
    assert proposal_cli.main(["accept", f"--actor={PERSON}", "--", made.id, made.digest]) == exits.OK
    assert (notes / "style.md").read_text() == made.text and "accepted" in capsys.readouterr().out
    [record] = (engine_home.root() / proposals.FOLDER / proposals.DECIDED).glob("p*.json")
    assert json.loads(record.read_text())["decision"]["by"] == PERSON


def test_ProposalCli_Reject_LeavesTheFolder(repo, notes, configured, capsys, monkeypatch):
    monkeypatch.setenv("SAKURA_ACTOR_TRUSTED", "1")  # the cockpit's own child carries the viewer's identity
    made = _proposed(repo, notes)
    assert proposal_cli.main(["reject", f"--actor={PERSON}", "--", made.id, made.digest]) == exits.OK
    assert (notes / "style.md").read_text() == "Use plain words.\n"


def test_Proposal_AcceptFromRoleSession_Refused(repo, notes, configured, monkeypatch, capsys):
    made = _proposed(repo, notes)
    monkeypatch.setenv(brakes.ROLE_ENV, "builder")
    for verb in ("accept", "reject"):
        assert proposal_cli.main([verb, "--", made.id, made.digest]) == exits.USAGE
    assert "cannot run inside an engine role" in capsys.readouterr().out
    assert (notes / "style.md").read_text() == "Use plain words.\n" and len(proposals.pending(engine_home.root())) == 1


def test_Proposal_AcceptFromInsideACage_Refused(repo, notes, configured, monkeypatch, capsys):
    made = _proposed(repo, notes)
    monkeypatch.setattr(engine_home, "sandboxed", lambda: True)
    assert proposal_cli.main(["accept", "--", made.id, made.digest]) == exits.USAGE
    assert len(proposals.pending(engine_home.root())) == 1


def test_ProposalCli_RefusalIsAUsageFailureWithTheReason(repo, notes, configured, capsys):
    made = _proposed(repo, notes)
    (notes / "style.md").write_text("changed\n")
    assert proposal_cli.main(["accept", "--", made.id, made.digest]) == exits.USAGE
    assert "changed since" in capsys.readouterr().out
    assert proposal_cli.main(["accept", "--", "p00000000", "0" * 12]) == exits.USAGE


def test_ProposalCli_FolderNowUnsafe_Refused(repo, notes, configured, capsys):
    made = _proposed(repo, notes)
    (notes / ".env").write_text("x")
    assert proposal_cli.main(["accept", "--", made.id, made.digest]) == exits.USAGE
    assert "credential" in capsys.readouterr().out and (notes / "style.md").read_text() == "Use plain words.\n"


def test_Brakes_EngineRoleRunsSakuraProposalAccept_Denied(repo):
    from test_flow_brakes import BUILDER, _bash, _denied

    make_context(repo)
    for verb in ("accept", "reject"):
        reason = _denied(repo, _bash(f"sakura proposal {verb} p01234567"), {**BUILDER, "SAKURA_FLOW_ROLE": "builder"})
        assert reason and "operator" in reason and f"proposal {verb}" in reason


# -- the waits --------------------------------------------------------------------------------------------------------


def test_Waits_PendingProposal_ShownAsAProposalWaitUntilDecided(repo, folders):
    made = _propose(repo, folders, text="Short words.\n", reason="Plain wording helps.")
    [wait] = [wait for wait in waits.collect([repo]) if wait.kind == waits.PROPOSAL]
    assert json.loads(wait.id) == ["wait", str(repo), "demo", "proposal", f"{made.id}@{made.digest}"]
    assert wait.title == "Change style.md in the shared knowledge folder" and wait.source == "flow"
    assert wait.since == "2026-11-16T12:00:00+00:00" and wait.author == "flow:builder"
    assert "Plain wording helps." in wait.detail and "Short words." in wait.detail
    proposals.reject(engine_home.root(), made.id, made.digest, PERSON, NOW)
    assert [wait for wait in waits.collect([repo]) if wait.kind == waits.PROPOSAL] == []


def test_Waits_NewFileProposal_TitleSaysAdd(repo, folders):
    _propose(repo, folders, path="topics/new.md")
    [wait] = [wait for wait in waits.collect([repo]) if wait.kind == waits.PROPOSAL]
    assert wait.title == "Add topics/new.md to the shared knowledge folder"


def test_ProposalWait_ShowsWholeText(repo, folders):
    text = ("line of words\n" * 400)[:proposals.MAX_TEXT]
    _propose(repo, folders, text=text)
    [wait] = [wait for wait in waits.collect([repo]) if wait.kind == waits.PROPOSAL]
    assert wait.detail.endswith(text) and "(cut)" not in wait.detail


def test_KnowledgePropose_LongerThanShown_Refused(repo, folders):
    for text in ("x" * (proposals.MAX_TEXT + 1), "\u00e9" * (proposals.MAX_TEXT // 2 + 1), "a\r\nb"):
        assert _call(_policy(folders, repo=repo), {**GOOD, "text": text}).startswith("refused")
    assert proposals.pending(engine_home.root()) == []
    assert _call(_policy(folders, repo=repo), {**GOOD, "text": "x" * proposals.MAX_TEXT}).startswith("recorded")


def test_ProposalAccept_DigestOfOtherText_Refused(repo, folders, notes, configured, capsys):
    made = _propose(repo, folders)
    other = "0" * 12 if made.digest != "0" * 12 else "1" * 12
    with pytest.raises(proposals.ProposalRefused, match="no longer waiting"):
        proposals.accept(engine_home.root(), made.id, other, folders, PERSON, NOW)
    assert proposal_cli.main(["accept", "--", made.id, other]) == exits.USAGE
    assert "no longer waiting" in capsys.readouterr().out
    assert (notes / "style.md").read_text() == "Use plain words.\n" and len(proposals.pending(engine_home.root())) == 1


# -- the tool a role calls ---------------------------------------------------------------------------------------------


def _policy(folders, tools=("knowledge_propose",), repo=None):
    return {"run": "build-1", "role": "builder", "requests": "/dev/null", "tools": list(tools),
            "knowledge": {"engine": str(engine_home.root()), "repo": str(repo), "task": "demo", "role": "builder",
                          "run": "build-1", "folders": {name: str(path) for name, path in folders.items()}}}


def _call(policy, arguments):
    [reply] = _session(policy, {"id": 1, "method": "tools/call",
                                   "params": {"name": "knowledge_propose", "arguments": arguments}})
    return reply["result"]["content"][0]["text"] if "result" in reply else reply["error"]["message"]


GOOD = {"folder": "shared", "path": "style.md", "text": "Use short words.\n", "reason": "Plain wording helps."}


def test_KnowledgePropose_Tool_RecordsProposalAuthoredByThePolicy(repo, folders):
    answer = _call(_policy(folders, repo=repo), GOOD)
    assert answer.startswith("recorded as p") and "written until the person accepts" in answer
    [found] = proposals.pending(engine_home.root())
    assert (found.author_kind, found.task, found.role, found.run) == ("role", "demo", "builder", "build-1")


@pytest.mark.parametrize("arguments", [{}, {**GOOD, "role": "evaluator"}, {**GOOD, "text": 5}, {"folder": "shared"},
                                       {**GOOD, "path": "../x.md"}, {**GOOD, "folder": "project"}])
def test_KnowledgePropose_Tool_BadArguments_Refused(repo, folders, arguments):
    assert _call(_policy(folders, repo=repo), arguments).startswith("refused")
    assert proposals.pending(engine_home.root()) == []


def test_KnowledgePropose_Tool_NotGranted_UnknownTool(repo, folders):
    assert "unknown tool" in _call(_policy(folders, tools=(), repo=repo), GOOD)


def test_Cage_DefaultRole_CanProposeIntoItsKnowledgeFolders(repo, notes):
    from sakura import repository

    root = engine_home.root()
    site = cage.RunSite(root / "proj" / "flow" / "demo", "build-1", "builder", repo, repository.common_dir(repo),
                        (root, engine_home.account_root()), repo, knowledge={"shared": str(notes.resolve())})
    run_cage = cage.prepare(site, cage.Policy("default"), 2, __import__("pathlib").Path("/home/someone"))
    policy = json.loads(run_cage.policy.read_text())
    assert "knowledge_propose" in policy["tools"]
    assert policy["knowledge"] == {"engine": str(root), "repo": str(repo), "task": "demo", "role": "builder",
                                   "run": "build-1", "folders": {"shared": str(notes.resolve())}}
    assert "mcp__sakura__knowledge_propose" in json.loads(run_cage.settings.read_text())["permissions"]["allow"]


# -- the cockpit: accept and reject are actions of a proposal wait ---------------------------------------------------


def _proposal_target(identifier="p01234567@0123456789ab"):
    from sakura.cockpit.wait_actions import WaitTarget

    repo = "/work/repo"
    key = json.dumps(["wait", repo, "demo", "proposal", identifier], separators=(",", ":"))
    return WaitTarget(waits.Wait(key, waits.PROPOSAL, repo, "repo", "demo", "Add a.md to the shared knowledge folder",
                                 "Reason: why", "2026-11-16T12:00:00+00:00", author="flow:builder"))


@pytest.mark.parametrize(("key", "verb"), [("y", "accept"), ("N", "reject")])
def test_CockpitActions_ProposalWait_RunsProposalCommandAsThePerson(key, verb):
    from sakura.actors import Actor
    from sakura.cockpit import actions

    actor = Actor("overlay", "owner@example.com", "nPHONE1", "Owner's phone")
    invocation = actions.plan(key, _proposal_target(), {}, actor)
    assert invocation.argv == [str(actions.PROGRAM), "proposal", verb, f"--actor={PERSON}", "--", "p01234567", "0123456789ab"]
    assert str(invocation.cwd) == "/work/repo" and actions.ACTIONS[key].confirm


@pytest.mark.parametrize("key", ["y", "N"])
@pytest.mark.parametrize("kind", ["approval", "owner_row", "question", "blocked"])
def test_CockpitActions_ProposalKeyOnAnotherWait_Refused(key, kind):
    from sakura.actors import Actor
    from sakura.cockpit import actions, wait_actions

    wait = _proposal_target().wait
    other = wait_actions.target_of(waits.Wait(wait.id.replace('"proposal"', f'"{kind}"'), kind, wait.repo, wait.project,
                                              wait.task, "t", "d", None))
    with pytest.raises(actions.ActionRefused):
        actions.plan(key, other, {}, Actor("overlay", "owner@example.com", "nPHONE1", "Owner's phone"))


def test_CockpitActions_ProposalWait_OffersAcceptRejectAndAttentionActions():
    from sakura.cockpit import actions

    assert sorted(action.key for action in actions.applicable(_proposal_target())) == ["N", "read", "snooze", "unread", "y"]


def test_CockpitPage_ProposalWait_GroupedByLaneWithActionsInDetail():
    from sakura.cockpit import gate, pages
    from test_cockpit_pages import CSRF, Audit, make_snapshot

    target = _proposal_target()
    snapshot = make_snapshot(waits=[target.wait])
    page = pages.index(snapshot, gate.Identity("o@example.com", "n1", "mac"), CSRF)
    assert page.index("<h3>Needs you</h3>") < page.index("<h4>demo</h4>") < page.index(target.wait.title)
    assert Audit(page).hidden_values("key") == ["read"]

    detail = pages.wait_view(snapshot, target.wait, CSRF)
    assert "<dd>Approval</dd>" in detail
    assert set(Audit(detail).hidden_values("key")) == {"y", "N", "read", "snooze"}
    assert ">Accept<" in detail and ">Reject<" in detail
