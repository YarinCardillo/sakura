"""Connectors: what is connected, the sign-in checks, and how each kind reads (every program, Keychain and network call is faked)."""

import io
import json
import os
import stat
from datetime import datetime, timezone

import pytest

from sakura import engine_home
from sakura.connectors import cli, kinds, registry
from sakura.digest import config, intake, state, supervise

NOW = datetime(2026, 11, 18, 9, 0, tzinfo=timezone.utc)
SECRET = "ntn_super_secret_token_value"


def done(stdout: str = "", code: int = 0) -> supervise.Execution:
    return supervise.Execution(code, stdout.encode(), "")


class Machine:
    """The programs, the Keychain and the network as the connectors see them."""

    def __init__(self, *, gh_signed_in=True, secrets=None):
        self.calls, self.posts = [], []
        self.gh_signed_in, self.secrets = gh_signed_in, secrets or {}
        self.gh_output = "[]"
        self.reply = b'{"results": []}'

    def run(self, argv, environment, timeout):
        self.calls.append(tuple(argv))
        if argv[:3] == ("gh", "auth", "status"):
            return done(code=0 if self.gh_signed_in else 1)
        return done(self.gh_output)

    def post(self, url, headers, body, timeout):
        self.posts.append((url, dict(headers), body))
        return self.reply

    def ports(self) -> kinds.Ports:
        return kinds.Ports(run=self.run, post=self.post, keychain=self.secrets.get, which=lambda name: name,
                           fetch_feed=lambda url, timeout: b"")


@pytest.fixture
def machine():
    return Machine()


def test_Connect_GithubNotSignedIn_IsRefusedWithAPlainReason(machine):
    machine.gh_signed_in = False
    with pytest.raises(registry.ConnectorError, match="gh is not signed in"):
        registry.connect("github", "", "", machine.ports(), NOW)
    assert registry.load() == []


def test_List_GithubSignedOut_IsConnectedFalseWithAReason(machine):
    registry.connect("github", "", "", machine.ports(), NOW)
    machine.gh_signed_in = False
    (found,) = registry.refresh(machine.ports(), NOW)
    assert found.connected is False and "gh auth login" in found.detail


def test_Connect_Notion_ReadsTheKeychainItemAndNeverStoresTheToken(machine):
    machine.secrets["sakura-connector-notion"] = SECRET
    found = registry.connect("notion", "", "", machine.ports(), NOW)
    assert found.connected is True and found.id == "notion"
    assert SECRET not in (registry.path().read_text() + json.dumps(registry.rows()))


def test_Connect_NotionWithoutAKeychainItem_NamesTheItem(machine):
    with pytest.raises(registry.ConnectorError, match="sakura-connector-notion"):
        registry.connect("notion", "", "", machine.ports(), NOW)


def test_Read_Notion_SendsTheTokenOnlyAsAHeaderAndNoErrorCarriesIt(machine):
    machine.secrets["sakura-connector-notion"] = SECRET
    registry.connect("notion", "", "", machine.ports(), NOW)
    machine.reply = json.dumps({"results": [
        {"id": "p1", "url": "https://notion.example/p1", "last_edited_time": "2026-11-18T08:00:00.000Z",
         "properties": {"Name": {"type": "title", "title": [{"plain_text": "Roadmap"}]}}},
        {"id": "p0", "url": "https://notion.example/p0", "last_edited_time": "2026-11-10T08:00:00.000Z",
         "properties": {}}]}).encode()
    fetched = kinds.read(registry.find("notion"), "2026-11-17T00:00:00+00:00", "", 20, machine.ports())
    assert [item.title for item in fetched.items] == ["Roadmap"] and fetched.cursor == "2026-11-18T08:00:00+00:00"
    url, headers, body = machine.posts[0]
    assert url == "https://api.notion.com/v1/search" and headers["Authorization"] == f"Bearer {SECRET}"
    assert SECRET not in body.decode()

    def failing(url, headers, body, timeout):
        raise kinds.ConnectorError("HTTP 401")

    ports = kinds.Ports(**{**machine.ports().__dict__, "post": failing})
    with pytest.raises(kinds.ConnectorError) as raised:
        kinds.read(registry.find("notion"), "", "", 20, ports)
    assert SECRET not in str(raised.value)


def test_Read_Github_ListsReviewRequestsAndAssignments(machine):
    registry.connect("github", "", "", machine.ports(), NOW)
    machine.gh_output = json.dumps([{"number": 7, "title": "Fix login", "url": "https://github.example/demo/app/pull/7",
                                     "repository": {"nameWithOwner": "demo/app"}, "updatedAt": "2026-11-18T07:00:00Z"}])
    fetched = kinds.read(registry.find("github"), "2026-11-18T00:00:00+00:00", "", 20, machine.ports())
    assert [item.title for item in fetched.items][0] == "demo/app #7 Fix login"
    assert any(call[:3] == ("gh", "search", "prs") for call in machine.calls)


def test_Read_Linear_UsesTheKeychainTokenAndIssueTitles(machine):
    machine.secrets["sakura-connector-linear"] = SECRET
    registry.connect("linear", "", "", machine.ports(), NOW)
    machine.reply = json.dumps({"data": {"viewer": {"assignedIssues": {"nodes": [
        {"identifier": "ENG-4", "title": "Speed up boot", "url": "https://linear.example/ENG-4",
         "updatedAt": "2026-11-18T08:30:00.000Z"}]}}}}).encode()
    fetched = kinds.read(registry.find("linear"), "2026-11-18T00:00:00+00:00", "", 20, machine.ports())
    assert [item.title for item in fetched.items] == ["ENG-4 Speed up boot"]
    assert machine.posts[0][1]["Authorization"] == SECRET


def test_Connect_ProgramOwnedByAnotherUser_IsRefused(tmp_path, machine, monkeypatch):
    program = tmp_path / "collect.sh"
    program.write_text("#!/bin/sh\n")
    program.chmod(0o700)
    real = os.stat(program)
    monkeypatch.setattr(kinds, "owner_of", lambda path: real.st_uid + 1)
    with pytest.raises(registry.ConnectorError, match="not owned by you"):
        registry.connect("program", "mine", str(program), machine.ports(), NOW)


def test_Connect_ProgramMustBeAbsoluteExecutableAndClosedToOthers(tmp_path, machine):
    with pytest.raises(registry.ConnectorError, match="absolute"):
        registry.connect("program", "mine", "collect.sh", machine.ports(), NOW)
    program = tmp_path / "open.sh"
    program.write_text("#!/bin/sh\n")
    program.chmod(0o777)
    with pytest.raises(registry.ConnectorError, match="writable by others"):
        registry.connect("program", "mine", str(program), machine.ports(), NOW)
    program.chmod(0o700)
    assert registry.connect("program", "mine", str(program), machine.ports(), NOW).id == "program-mine"


def test_Connect_FeedNeedsAnHttpAddress(machine):
    with pytest.raises(registry.ConnectorError, match="http"):
        registry.connect("feed", "news", "ftp://example.com/feed", machine.ports(), NOW)
    assert registry.connect("feed", "news", "https://example.com/feed.xml", machine.ports(), NOW).id == "feed-news"


def test_Disconnect_RemovesTheConnectorAndNothingElse(machine):
    registry.connect("github", "", "", machine.ports(), NOW)
    machine.secrets["sakura-connector-linear"] = SECRET
    registry.connect("linear", "", "", machine.ports(), NOW)
    registry.disconnect("github")
    assert [found.id for found in registry.load()] == ["linear"]
    with pytest.raises(registry.ConnectorError, match="no connector"):
        registry.disconnect("github")


def test_Registry_File_IsPrivateToThePerson(machine):
    registry.connect("github", "", "", machine.ports(), NOW)
    assert stat.S_IMODE(registry.path().stat().st_mode) == 0o600


def test_Cli_ListJson_ShowsTheFieldsTheAppReads(machine):
    registry.connect("github", "", "", machine.ports(), NOW)
    out = io.StringIO()
    assert cli.main(["list", "--json"], stdout=out, ports=machine.ports(), now=lambda: NOW) == 0
    (row,) = json.loads(out.getvalue())
    assert set(row) == {"id", "kind", "label", "connected", "detail", "last_used_at"} and row["id"] == "github"


def _program(tmp_path, name="pull-requests"):
    path = tmp_path / name
    path.write_text("#!/bin/sh\n")
    path.chmod(0o700)
    return path


def _digest_reads(program, extra=""):
    settings = engine_home.root() / config.CONFIG_FILE
    settings.write_text(f'[[source]]\nid = "work-github"\nrun = ["{program}"]\n{extra}')
    settings.chmod(0o600)


def test_Rows_AProgramTheDigestReads_IsListedLikeAConnector(tmp_path, machine):
    _digest_reads(_program(tmp_path))
    state.save(state.State({"work-github": state.SourceState("c", "2026-11-18T08:00:00+00:00")}))
    (row,) = registry.rows()
    assert row == {"id": "work-github", "kind": "program", "label": "work-github", "connected": True,
                   "detail": "the program is in place", "last_used_at": "2026-11-18T08:00:00+00:00"}


def test_Rows_AProgramWithALabelInTheDigestFile_IsListedUnderThatName(tmp_path, machine):
    _digest_reads(_program(tmp_path), extra='label = "Team pull requests"\n')
    (row,) = registry.rows()
    assert (row["id"], row["label"]) == ("work-github", "Team pull requests")


def test_Rows_AProgramThatCannotBeRun_IsListedNotConnectedWithTheReason(tmp_path, machine):
    _digest_reads(tmp_path / "missing")
    (row,) = registry.rows()
    assert (row["id"], row["connected"], row["detail"], row["last_used_at"]) == (
        "work-github", False, "the program is not an executable file", None)


def test_Rows_FeedsAndConnectorSourcesOfTheDigest_AreNotListedTwice(tmp_path, machine):
    registry.connect("github", "", "", machine.ports(), NOW)
    settings = engine_home.root() / config.CONFIG_FILE
    settings.write_text('[[source]]\nid = "code"\nconnector = "github"\n[[source]]\nid = "news"\nfeed = "https://example.test/rss"\n')
    settings.chmod(0o600)
    assert [row["id"] for row in registry.rows()] == ["github"]


def test_Rows_ADigestFileThatDoesNotLoad_LeavesTheConnectorsAlone(machine):
    registry.connect("github", "", "", machine.ports(), NOW)
    settings = engine_home.root() / config.CONFIG_FILE
    settings.write_text("not = [valid")
    settings.chmod(0o600)
    assert [row["id"] for row in registry.rows()] == ["github"]


def test_Cli_ConnectNeverTakesASecretOnTheCommandLine(machine):
    out = io.StringIO()
    with pytest.raises(SystemExit):
        cli.main(["connect", "notion", "--token", SECRET], stdout=out, ports=machine.ports(), now=lambda: NOW)


def test_Intake_ASourceNamingAConnector_RunsThroughIt(machine):
    registry.connect("github", "", "", machine.ports(), NOW)
    machine.gh_output = json.dumps([{"number": 7, "title": "Fix login", "url": "https://github.example/demo/app/pull/7",
                                     "repository": {"nameWithOwner": "demo/app"}, "updatedAt": "2026-11-18T08:00:00Z"}])
    source = config.Source("code", connector="github")
    settings = config.DigestConfig("07:30", "sonnet", "en", 24, (source,))
    (result,) = intake.collect(settings, state.State(), NOW, connect=lambda *arguments: kinds.read(*arguments, machine.ports()))
    assert result.report.ok and result.items[0].title == "demo/app #7 Fix login"
    assert registry.find("github").last_used_at == "2026-11-18T09:00:00Z"


def test_Intake_AConnectorThatIsNotConnected_FailsOnlyItsSource(machine):
    source = config.Source("code", connector="github")
    settings = config.DigestConfig("07:30", "sonnet", "en", 24, (source,))
    (result,) = intake.collect(settings, state.State(), NOW, connect=lambda *arguments: kinds.read(*arguments, machine.ports()))
    assert result.report.ok is False and "not connected" in result.report.error


def test_ReadKeychain_AsksSecurityForTheItemAndReturnsNoneWhenItIsMissing():
    seen = []

    def fake(argv, environment, timeout):
        seen.append(argv)
        return done(SECRET + "\n") if len(seen) == 1 else done("", 44)

    assert kinds.read_keychain("sakura-connector-notion", fake) == SECRET
    assert kinds.read_keychain("sakura-connector-notion", fake) is None
    assert seen[0] == ("/usr/bin/security", "find-generic-password", "-s", "sakura-connector-notion", "-w")


def test_digest_requires_reconnection_after_the_program_installation_changes(tmp_path, machine):
    executable = _program(tmp_path)
    connected = registry.connect("program", "mine", str(executable), machine.ports(), NOW)
    executable.write_text("#!/bin/sh\n# new installed version\n")
    calls = []

    def run(*arguments):
        calls.append(arguments)
        return supervise.Execution(0, b"", "")

    result = intake.run_source(config.Source("test", connector=connected.id), "", "", run=run)

    assert not result.report.ok and calls == []


def test_versioned_instances_preserve_legacy_ids_and_rename_in_place(machine):
    registry.path().write_text(json.dumps({"connectors": [{"id": "github", "kind": "github", "label": "Old"},
                                                         {"id": "feed-news", "kind": "feed", "label": "News",
                                                          "target": "https://example.test/feed"}]}))
    fields = {"kind": "feed", "label": "News", "address": "https://example.test/a"}
    first = registry.mutate("add", "one", machine.ports(), NOW, fields=fields)
    assert registry.mutate("add", "one", machine.ports(), NOW, fields=fields) == first
    second = registry.mutate("add", "two", machine.ports(), NOW, fields={**fields, "address": "https://example.test/b"})
    assert len({first["id"], second["id"], "feed-news"}) == 3
    edited = registry.mutate("edit", "rename", machine.ports(), NOW, connector_id=first["id"],
                             revision=first["revision"], fields={"label": "Renamed"})
    assert edited == {"id": first["id"], "revision": 2}
    assert registry.find(first["id"]).target == fields["address"]
    assert [row.id for row in registry.load()][:2] == ["github", "feed-news"]
    assert json.loads(registry.path().read_text())["version"] == 1


def test_revocation_wins_over_late_refresh_and_stale_form(machine):
    fields = {"kind": "feed", "label": "News", "address": "https://example.test/a"}
    first = registry.mutate("add", "one", machine.ports(), NOW, fields=fields)
    connector_id = first["id"]
    before = registry.find(connector_id)
    registry.mutate("revoke", "two", machine.ports(), NOW, connector_id=connector_id, revision=1)
    registry.mark_used(connector_id, NOW, revision=before.revision, is_ok=True)
    registry.refresh(machine.ports(), NOW)
    after = registry.find(connector_id)
    assert not after.connected and after.revoked and after.last_used_at is None
    with pytest.raises(ValueError, match="stale_revision"):
        registry.mutate("edit", "three", machine.ports(), NOW, connector_id=connector_id, revision=1, fields=fields)


def test_concurrent_adds_and_interrupted_write_keep_registry_consistent(machine, monkeypatch):
    from concurrent.futures import ThreadPoolExecutor

    fields = {"kind": "feed", "label": "News", "address": "https://example.test/a"}
    with ThreadPoolExecutor(2) as pool:
        results = list(pool.map(lambda op: registry.mutate("add", op, machine.ports(), NOW, fields=fields),
                                ("one", "two")))
    assert len(registry.load()) == len({row["id"] for row in results}) == 2

    def fail(*args):
        raise OSError("interrupted")

    with monkeypatch.context() as patch:
        patch.setattr(registry.gates.os, "replace", fail)
        with pytest.raises(OSError, match="interrupted"):
            registry.mutate("add", "three", machine.ports(), NOW, fields=fields)
    assert len(registry.load()) == 2
    registry.mutate("add", "three", machine.ports(), NOW, fields=fields)
    assert len(registry.load()) == 3


def test_scoped_read_rejects_sign_in_drift(machine):
    from dataclasses import replace

    principal = ["account-1"]
    ports = replace(machine.ports(), inspect=lambda *args: kinds.Access(principal[0], "repo-1", "Acme/Web"))
    receipt = registry.mutate("add", "one", ports, NOW, fields={"kind": "github", "label": "Code",
                                                               "repo": "acme/web", "branch": "staging"})
    found = registry.find(receipt["id"])
    principal[0] = "account-2"
    with pytest.raises(kinds.ConnectorError, match="reconnect_required"):
        kinds.read(found, "", "", 10, ports)
    assert not machine.calls


@pytest.mark.parametrize(("kind", "fields", "records"), [
    ("github", {"repo": "acme/web", "branch": "staging"},
     [{"item": {"id": "g1", "title": "Other repository"}, "resource": "foreign", "branch": "staging"},
      {"item": {"id": "g2", "title": "Other base"}, "resource": "scope-1", "branch": "main"},
      {"item": {"id": "g3", "title": "Issue"}, "resource": "scope-1", "is_issue": True},
      {"item": {"id": "ok", "title": "Matching PR"}, "resource": "scope-1", "branch": "staging"}]),
    ("linear", {"team": "ENG"}, [{"item": {"id": "bad", "title": "Foreign team"}, "resource": "foreign"},
                                {"item": {"id": "ok", "title": "Matching issue"}, "resource": "scope-1"}]),
    ("notion", {"database": "db"}, [{"item": {"id": "bad", "title": "Foreign database"}, "resource": "foreign"},
                                   {"item": {"id": "ok", "title": "Matching page"}, "resource": "scope-1"}]),
])
def test_scoped_reads_filter_verified_membership(kind, fields, records, machine):
    from dataclasses import replace

    ports = replace(machine.ports(), inspect=lambda *args: kinds.Access("principal", "scope-1", "Scope"),
                    page=lambda *args: kinds.Page(records))
    receipt = registry.mutate("add", "one", ports, NOW, fields={"kind": kind, "label": "Scoped", **fields})
    result = kinds.read(registry.find(receipt["id"]), "", "", 10, ports)
    assert [item.id for item in result.items] == ["ok"]


def test_scoped_page_failure_and_missing_membership_withhold_batch(machine):
    from dataclasses import replace

    def next_page(connector, access, after, remaining):
        if after:
            raise kinds.ConnectorError("provider body with private data")
        return kinds.Page([{"item": {"id": "ok", "title": "One"}, "resource": "scope-1"}], "page-two")

    ports = replace(machine.ports(), inspect=lambda *args: kinds.Access("principal", "scope-1", "Scope"), page=next_page)
    result = registry.mutate("add", "one", ports, NOW, fields={"kind": "linear", "label": "Team", "team": "ENG"})
    found = registry.find(result["id"])
    with pytest.raises(kinds.ConnectorError, match="^connector_unavailable$"):
        kinds.read(found, "", "old", 10, ports)
    ports = replace(ports, page=lambda *args: kinds.Page([{"item": {"id": "bad", "title": "Unknown team"}}]))
    with pytest.raises(kinds.ConnectorError, match="^scope_unavailable$"):
        kinds.read(found, "", "old", 10, ports)


def test_out_of_scope_or_unpermitted_write_never_dispatches(machine):
    from dataclasses import replace

    writes = []
    operations = ["comment_pr"]
    membership = {"resource": "scope-1", "branch": "main", "target": "7"}
    ports = replace(machine.ports(), inspect=lambda *args: kinds.Access("principal", "scope-1", "Scope", tuple(operations)),
                    lookup=lambda *args: membership, send=lambda *args: writes.append(args))
    receipt = registry.mutate("add", "one", ports, NOW, fields={"kind": "github", "label": "Code", "repo": "org/repo",
                                                               "branch": "staging"})
    found = registry.find(receipt["id"])
    with pytest.raises(kinds.ConnectorError, match="scope_unavailable"):
        kinds.write(found, "comment_pr", "7", "Reviewed", 10, ports)
    membership["branch"] = "staging"
    operations.clear()
    with pytest.raises(kinds.ConnectorError, match="connector_unavailable"):
        kinds.write(found, "comment_pr", "7", "Reviewed", 10, ports)
    assert writes == []
    operations.append("comment_pr")
    kinds.write(found, "comment_pr", "7", "Reviewed", 10, ports)
    assert len(writes) == 1


def test_replaced_custom_executable_requires_new_approval(machine, tmp_path):
    path = _program(tmp_path)
    result = registry.mutate("add", "one", machine.ports(), NOW, fields={
        "kind": "program", "label": "Reader", "reads": "Reads items", "program": str(path), "args": ["--one"]})
    path.write_text("#!/bin/sh\nexit 1\n")
    found = registry.find(result["id"])
    with pytest.raises(kinds.ConnectorError, match="approval_required"):
        kinds.read(found, "", "", 10, machine.ports())
    assert not machine.calls


def test_github_live_adapter_pins_host_account_and_base_filter(machine):
    from dataclasses import replace

    calls = []

    def run(argv, environment, timeout):
        calls.append(argv)
        if argv[1:3] == ("auth", "token"):
            return done("fake-token")
        assert environment["GH_ENTERPRISE_TOKEN"] == "fake-token"
        assert ("--hostname", "code.example.test") == argv[2:4]
        path = argv[-1]
        if path == "user":
            return done("HTTP/2 200\nX-OAuth-Scopes: repo\n\n" + json.dumps({"id": 31}))
        if path in ("repos/acme/web", "repositories/42"):
            return done(json.dumps({"id": 42, "full_name": "acme/web", "permissions": {"pull": True},
                                    "private": True, "archived": False}))
        if path == "repos/acme/web/branches/staging":
            return done("{}")
        assert path.startswith("repos/acme/web/pulls?") and "base=staging" in path
        return done(json.dumps([{"number": 1, "title": "PR", "base": {"ref": "staging", "repo": {"id": 42}},
                                 "html_url": "https://code.example.test/acme/web/pull/1",
                                 "updated_at": "2026-11-18T08:00:00Z"}]))

    ports = replace(machine.ports(), run=run)
    receipt = registry.mutate("add", "add", ports, NOW, fields={"kind": "github", "label": "Web",
                                                              "repo": "code.example.test/acme/web", "branch": "staging"})
    found = registry.find(receipt["id"])
    assert found.principal == "code.example.test:31" and kinds.operation_keys(found) == ("read", "comment_pr")
    assert [item.title for item in kinds.read(found, "", "", 10, ports).items] == ["PR"]
    assert "fake-token" not in registry.path().read_text()
