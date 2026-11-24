"""The cage the engine writes for every run: Claude Code settings, the Seatbelt profile, the prompt server."""

import dataclasses
import io
import json
import os
import re
import shlex
import shutil
import socket
import subprocess
import sys
from pathlib import Path

import pytest

from conftest import git
from sakura import engine_home, repository
from sakura.flow import cage, cage_server, contract, store
from sakura.flow.surface import KIT_BIN
from flow_support import make_context

HOME = Path("/home/someone")
seatbelt_integration = pytest.mark.skipif(
    os.environ.get("SAKURA_TEST_SEATBELT") != "1",
    reason="set SAKURA_TEST_SEATBELT=1 to run the native sandbox checks",
)


def _site(repo, role="builder", run="build-1"):
    roots = (engine_home.root(), engine_home.account_root())
    return cage.RunSite(engine_home.root() / "proj" / "flow" / "demo", run, role, repo, repository.common_dir(repo),
                        roots, repo)


def _prepared(repo, role="builder", policy=None, subagents=2, home=HOME):
    return cage.prepare(_site(repo, role), policy or cage.Policy("default"), subagents, home)


# -- Claude Code settings -------------------------------------------------------------------------


def test_claude_settings_turn_the_sandbox_on_strictly(repo):
    run_cage = _prepared(repo)
    sandbox = json.loads(run_cage.settings.read_text())["sandbox"]
    assert (sandbox["enabled"], sandbox["allowUnsandboxedCommands"], sandbox["failIfUnavailable"]) == (True, False, True)
    assert sandbox["allowAppleEvents"] is False and sandbox["autoAllowBashIfSandboxed"] is True
    assert sandbox["network"]["strictAllowlist"] is True


def test_claude_settings_deny_secret_and_engine_reads_and_allow_only_the_workspace_writes(repo):
    settings = json.loads(_prepared(repo).settings.read_text())
    denied = settings["sandbox"]["filesystem"]["denyRead"]
    for relative in (".ssh", ".aws", ".gnupg", ".config/gh", ".claude", ".codex", "Library/Keychains"):
        assert str(HOME / relative) in denied
    assert str(engine_home.root()) in denied and str(HOME / "**/.env*") in denied
    assert str(repo / "**/.env*") in denied
    common = repository.common_dir(repo)
    assert str(common / "hooks") in settings["sandbox"]["filesystem"]["denyWrite"]
    assert settings["permissions"]["blockWritesOutsideWorkingDirectories"] is True
    # The read block would hide the engine's own `sakura` from sandboxed commands (measured), so reads are
    # limited by the deny lists instead.
    assert "blockReadsOutsideWorkingDirectories" not in settings["permissions"]


def test_claude_settings_deny_the_programs_that_reach_outside(repo):
    deny = json.loads(_prepared(repo).settings.read_text())["permissions"]["deny"]
    for program in ("osascript", "open", "launchctl", "tailscale", "docker", "kill", "pkill", "killall", "security",
                    "ps", "pbpaste", "pbcopy"):
        assert f"Bash({program} *)" in deny and f"Bash({program})" in deny
    assert "Bash(git push *)" in deny
    assert f"Read(/{HOME / '.ssh'}/**)" in deny and f"Edit(/{engine_home.root()}/**)" in deny
    assert "--dangerously-skip-permissions" not in json.dumps(deny)


def test_policy_secret_paths_hosts_and_registries_are_added(repo):
    (repo / "package.json").write_text("{}")
    policy = cage.Policy("canary", secret_paths=("~/.canary",), allowed_domains=("example.org",))
    settings = json.loads(_prepared(repo, policy=policy).settings.read_text())
    assert str(HOME / ".canary") in settings["sandbox"]["filesystem"]["denyRead"]
    hosts = settings["sandbox"]["network"]["allowedDomains"]
    assert hosts[:2] == ["registry.npmjs.org", "registry.yarnpkg.com"]
    assert "api.github.com" in hosts and "api.anthropic.com" in hosts and hosts[-1] == "example.org"
    assert "github.com" not in hosts


def test_reviewers_cannot_edit_and_no_subagents_means_no_agent_tool(repo):
    deny = json.loads(_prepared(repo, role="code_reviewer", subagents=0).settings.read_text())["permissions"]["deny"]
    assert {"Edit", "Write", "NotebookEdit", "Agent", "Task"} <= set(deny)
    builder = json.loads(_prepared(repo).settings.read_text())["permissions"]["deny"]
    assert "Edit" not in builder and "Agent" not in builder


def test_cage_files_live_in_the_task_folder_and_are_rewritten(repo):
    run_cage = _prepared(repo)
    assert repo not in run_cage.settings.parents and run_cage.folder.name == "build-1"
    run_cage.requests.write_text("stale\n")
    run_cage.settings.write_text("{}")
    again = _prepared(repo)
    assert not again.requests.exists() and json.loads(again.settings.read_text())["sandbox"]["enabled"]


def test_mcp_config_names_only_the_engine_server(repo):
    servers = json.loads(_prepared(repo).mcp_config.read_text())["mcpServers"]
    assert servers == {"sakura": {"command": str(KIT_BIN), "args": ["cage", "serve", str(_prepared(repo).policy)]}}


def test_policy_files_extend_the_default_and_fail_closed(repo):
    folder = engine_home.root() / cage.POLICIES_DIR
    folder.mkdir()
    (folder / "canary.toml").write_text('secret_paths = ["~/.x"]\nallowed_domains = ["a.example"]\n')
    policy = cage.load_policy("canary")
    assert (policy.secret_paths, policy.allowed_domains) == (("~/.x",), ("a.example",))
    for text, message in (("colour = 1\n", "unknown keys"), ('secret_paths = ["rel"]\n', "absolute"),
                          ('stack = "ask"\n', "remove the stack key"), ("local_network = 1\n", "true or false")):
        (folder / "bad.toml").write_text(text)
        with pytest.raises(cage.CageError, match=message):
            cage.load_policy("bad")
    with pytest.raises(cage.CageError, match="unknown cage policy"):
        cage.load_policy("ghost")


# -- the Seatbelt profile of exec roles and contract checks, measured --------------------------------


def _caged(run_cage, command, cwd, extra_env=None):
    env = {**{key: value for key, value in os.environ.items() if not key.startswith("SAKURA_FLOW_")},
           **run_cage.env, **(extra_env or {})}
    return subprocess.run(run_cage.wrap(["sh", "-c", command]), cwd=cwd, env=env, capture_output=True, text=True,
                          timeout=60, stdin=subprocess.DEVNULL, check=False)


def test_seatbelt_profile_declares_read_write_and_system_denials(repo, tmp_path):
    secret = tmp_path / "secret"
    secret.mkdir()
    installed = _prepared(repo, policy=cage.Policy("canary", secret_paths=(str(secret),)))
    lines = installed.seatbelt.read_text().splitlines()
    denied_reads = [line for line in lines if line.startswith("(deny file-read* ")]
    denied_writes = [line for line in lines if line.startswith("(deny file-write* ")]
    assert any(str(secret) in line for line in denied_reads)
    assert any(str(engine_home.root()) in line for line in denied_reads)
    assert any(str(repository.common_dir(repo) / "hooks") in line for line in denied_writes)
    assert {"(deny network*)", "(deny signal)", "(deny appleevent-send)"} <= set(lines)
    assert any(line.startswith("(deny process-exec ") and '"/bin/launchctl"' in line for line in lines)


@seatbelt_integration
def test_seatbelt_profile_blocks_what_the_cage_denies(repo, tmp_path):
    secret = tmp_path / "secret"
    secret.mkdir()
    (secret / "s.txt").write_text("canary")
    outside = tmp_path / "outside"
    outside.mkdir()
    canary = subprocess.Popen(["sleep", "60"])
    try:
        run_cage = _prepared(repo, policy=cage.Policy("canary", secret_paths=(str(secret),)), home=Path.home())
        task = engine_home.root() / "proj" / "flow" / "demo"
        for name in ("contract.json", "ledger.md"):
            (task / name).write_text("engine state")
        (engine_home.root() / "config.toml").write_text("")
        probes = {
            "read secret": f"cat {secret / 's.txt'}",
            "read engine state": f"ls {engine_home.root()}/proj",
            "read the task contract": f"cat {engine_home.root()}/proj/flow/demo/contract.json",
            "read the ledger": f"cat {engine_home.root()}/proj/flow/demo/ledger.md",
            "read engine config": f"cat {engine_home.root()}/config.toml",
            "write outside": f"echo x > {outside / 'o.txt'}",
            "write engine home": f"echo x > {engine_home.root() / 'x.txt'}",
            "write git hooks": f"echo x > {repository.common_dir(repo) / 'hooks' / 'pre-commit'}",
            "signal": f"kill -0 {canary.pid}",

            "apple events": "osascript -e 'return 1'",
            "launchd": "launchctl list",
        }
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            address = listener.getsockname()
            with socket.create_connection(address, timeout=5):
                pass
            script = f"import socket; socket.create_connection({address!r}, timeout=5).close()"
            assert _caged(run_cage, shlex.join([sys.executable, "-c", script]), repo).returncode != 0
        for name, command in probes.items():
            assert _caged(run_cage, command, repo).returncode != 0, name
        assert canary.poll() is None
        assert _caged(run_cage, "echo x > inside.txt && git add inside.txt && git commit -qm caged", repo).returncode == 0
        assert not (outside / "o.txt").exists()
    finally:
        canary.kill()
        canary.wait()


PROCARGS = """import ctypes, subprocess, sys
libc = ctypes.CDLL(None, use_errno=True)
def readable(pid):
    mib = (ctypes.c_int * 3)(1, 49, pid)  # CTL_KERN, KERN_PROCARGS2: a process's arguments and environment
    size = ctypes.c_size_t(1 << 20)
    buffer = ctypes.create_string_buffer(size.value)
    return libc.sysctl(mib, 3, buffer, ctypes.byref(size), None, 0) == 0 and buffer.raw[:size.value]
other = readable(int(sys.argv[1]))
child = subprocess.Popen(["sleep", "5"])
print("other", "canary" if other and sys.argv[2].encode() in other else "hidden")
print("child", "readable" if readable(child.pid) else "denied")
child.kill()
"""
BOOTSTRAP = """import ctypes, sys
libc = ctypes.CDLL(None)
port = ctypes.c_uint.in_dll(libc, "bootstrap_port")
for name in sys.argv[1:]:
    print(name, libc.bootstrap_look_up(port, name.encode(), ctypes.byref(ctypes.c_uint(0))))
"""
SENSITIVE_SERVICES = ("com.apple.pasteboard.1", "com.apple.SecurityServer", "com.apple.coreservices.launchservicesd",
                      "com.apple.lsd.mapdb")


def test_Seatbelt_ACagedCommand_SeesNoOtherProcessEnvironmentPasteboardKeychainOrLaunchServices(repo):
    canary_text = "ps-canary-6c4"
    canary = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"],
                              env={**os.environ, "SAKURA_PS_CANARY": canary_text})
    try:
        run_cage = _prepared(repo, home=Path.home())
        (repo / "procargs.py").write_text(PROCARGS)
        (repo / "bootstrap.py").write_text(BOOTSTRAP)
        listing = _caged(run_cage, "ps eww -A", repo)
        assert canary_text not in listing.stdout
        seen = _caged(run_cage, f"{sys.executable} -I procargs.py {canary.pid} {canary_text}", repo)
        assert seen.stdout.split("\n")[:2] == ["other hidden", "child readable"], seen.stdout + seen.stderr
        # Only exit statuses: the pasteboard and the keychain are never printed.
        for name, command in {"pasteboard": "pbpaste > /dev/null 2>&1", "keychain": "security find-generic-password > /dev/null 2>&1",
                              "launch an app": "open -g -a Finder > /dev/null 2>&1"}.items():
            assert _caged(run_cage, command, repo).returncode != 0, name
        lookups = _caged(run_cage, f"{sys.executable} -I bootstrap.py {' '.join(SENSITIVE_SERVICES)}", repo)
        answers = dict(line.rsplit(" ", 1) for line in lookups.stdout.splitlines())
        assert set(answers) == set(SENSITIVE_SERVICES) and "0" not in answers.values(), lookups.stdout + lookups.stderr
    finally:
        canary.kill()
        canary.wait()


def test_Cage_TheRowRunsFolder_IsOutOfReachOfEveryOtherCage(repo):
    build_dir = cage.new_build_dir()
    try:
        (build_dir / "pycache").mkdir()
        lane = _prepared(repo, home=Path.home())
        assert _caged(lane, f"echo planted > {build_dir / 'pycache' / 'mod.pyc'}", repo).returncode != 0
        assert _caged(lane, f"ls {build_dir}", repo).returncode != 0
        settings = json.loads(lane.settings.read_text())["sandbox"]["filesystem"]
        assert str(cage.rows_root()) in settings["denyRead"] and str(cage.rows_root()) in settings["denyWrite"]
    finally:
        import shutil

        shutil.rmtree(build_dir, ignore_errors=True)


def _inside_any(path, folders):
    real = Path(os.path.realpath(path))
    return any(real == Path(os.path.realpath(folder)) or Path(os.path.realpath(folder)) in real.parents
               for folder in folders)


def test_Cage_TheEnginesObjectStore_IsInNoCagesWriteSet(repo):
    from sakura.flow import checks, sealed

    # The task's store trusts the repository a seal verifies its source against; the task has its own repository.
    ctx = make_context(repo, own_repository=True)
    store_path = sealed.seal(repo, git(repo, "rev-parse", "HEAD"))
    build_dir = cage.new_build_dir()
    try:
        cages = {"lane": _prepared(repo, home=Path.home()),
                 "checks": checks.checks_cage(ctx.store, repo, "checks-store", cage.Policy("default")),
                 "row run": checks.checks_cage(ctx.store, repo, "rows-store", cage.Policy("default"),
                                               build_dir=build_dir)}
        for name, run_cage in cages.items():
            planted = store_path / "objects" / f"planted-{name.replace(' ', '-')}"
            done = _caged(run_cage, f"echo x > {planted} || exit 1; mkdir {store_path}/refs/sealed/x", repo)
            assert done.returncode != 0 and not planted.exists(), name
            settings = json.loads(run_cage.settings.read_text())["sandbox"]["filesystem"]
            assert not _inside_any(store_path, settings["allowWrite"]), name
            assert _inside_any(store_path, settings["denyWrite"]) and _inside_any(store_path, settings["denyRead"]), name
    finally:
        import shutil

        shutil.rmtree(build_dir, ignore_errors=True)


def test_operator_command_from_a_caged_lane_is_refused_by_the_sandbox(repo):
    ctx = make_context(repo, state={"workspaces": {"0": {"path": str(repo), "branch": "main", "ports": [1, 2]}},
                                    "contract_sha256": ""})
    contract.save(ctx.store, [contract.Row("R1", "j", "s", "cli", None, "try it", False, False, "owner")])
    run_cage = _prepared(repo, home=Path.home())
    done = _caged(run_cage, f"{KIT_BIN} flow mark demo R1 ok", repo)
    assert done.returncode != 0
    assert "operator commands cannot run inside an agent cage" in done.stdout, done.stdout + done.stderr
    fresh = store.TaskStore(repo, "demo")
    assert not contract.load(fresh, fresh.read_state()["contract_sha256"])[0].passes


def test_operator_commands_refuse_when_the_engine_home_is_not_writable(repo, monkeypatch, capsys):
    from sakura.flow import cli

    monkeypatch.setattr(engine_home, "sandboxed", lambda: True)
    monkeypatch.chdir(repo)
    for argv in (["mark", "demo", "R1", "ok"], ["approve", "demo", "spec"], ["stop", "demo"]):
        assert cli.main(argv) != 0
        assert "inside an agent cage" in capsys.readouterr().out


def test_sandbox_probe_is_false_outside_a_cage():
    assert engine_home.sandboxed() is False


# -- the prompt server ------------------------------------------------------------------------------


def _policy(repo, tmp_path, **overrides):
    policy = {"run": "build-1", "role": "builder", "requests": str(tmp_path / "requests.jsonl")}
    return {**policy, **overrides}


@pytest.mark.parametrize("tool, tool_input", [
    ("Edit", {"file_path": "/anywhere/a.py"}), ("Read", {"file_path": "/etc/hosts"}),
    ("WebFetch", {"url": "https://registry.npmjs.org/x"}), ("WebFetch", {"url": "http://allowed.com\\@evil.com"}),
    ("WebFetch", {"url": "http://evil.com#@allowed.com"}), ("Bash", {"command": "ls"}), ("Agent", {}),
    ("mcp__github__push", {})])
def test_server_denies_every_prompt_and_parses_nothing(repo, tmp_path, tool, tool_input):
    # What the cage allows is in Claude Code's own rules; a prompt that reaches the server is outside them.
    assert cage_server.decide(_policy(repo, tmp_path), tool, tool_input)[0] == "deny"


def test_allowed_hosts_are_native_webfetch_rules_never_parsed_urls(repo):
    allow = json.loads(_prepared(repo).settings.read_text())["permissions"]["allow"]
    assert "WebFetch(domain:api.github.com)" in allow and "WebFetch(domain:api.anthropic.com)" in allow
    assert not any("evil" in rule or "@" in rule for rule in allow)
    allowed = json.loads(_prepared(repo).settings.read_text())["sandbox"]["filesystem"]["allowRead"]
    assert not any(str(root) in path for path in allowed for root in (engine_home.root(), engine_home.account_root()))


def _session(policy, *messages):
    out = io.StringIO()
    cage_server.serve(policy, io.StringIO("".join(json.dumps(message) + "\n" for message in messages)), out)
    return [json.loads(line) for line in out.getvalue().splitlines()]


def test_server_speaks_mcp_and_answers_with_one_text_block(repo, tmp_path):
    policy = _policy(repo, tmp_path)
    replies = _session(policy,
                       {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}},
                       {"jsonrpc": "2.0", "method": "notifications/initialized"},
                       {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
                       {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "permission", "arguments": {
                           "tool_name": "WebFetch", "input": {"url": "https://evil.test/a"}, "tool_use_id": "t"}}},
                       {"jsonrpc": "2.0", "id": 4, "method": "nope"})
    assert [reply["id"] for reply in replies] == [1, 2, 3, 4]
    assert replies[0]["result"]["protocolVersion"] == "2025-06-18"
    assert {tool["name"] for tool in replies[1]["result"]["tools"]} == {"permission"}
    content = replies[2]["result"]["content"]
    assert len(content) == 1 and content[0]["type"] == "text" and "structuredContent" not in replies[2]["result"]
    answer = json.loads(content[0]["text"])
    assert answer["behavior"] == "deny" and "network allow-list" in answer["message"]
    assert replies[3]["error"]["code"] == -32601
    recorded = [json.loads(line) for line in Path(policy["requests"]).read_text().splitlines()]
    assert recorded[0]["tool"] == "WebFetch" and "detail" not in recorded[0]


def test_a_prompt_for_a_workspace_edit_is_still_denied_and_recorded(repo, tmp_path):
    policy = _policy(repo, tmp_path)
    replies = _session(policy, {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
        "name": "permission", "arguments": {"tool_name": "Edit", "input": {"file_path": str(repo / "a")}}}})
    assert json.loads(replies[0]["result"]["content"][0]["text"])["behavior"] == "deny"
    assert "detail" not in json.loads(Path(policy["requests"]).read_text())


ROW_ENV = {"SAKURA_FLOW_TASK": "demo", "SAKURA_FLOW_PHASE": "evaluate", "SAKURA_FLOW_STACK": "demo:0"}


def test_only_the_evaluator_gets_the_run_row_tool(repo):
    roots = (engine_home.root(), engine_home.account_root())
    evaluator = cage.prepare(cage.RunSite(engine_home.root() / "proj" / "flow" / "demo", "evaluate-1", "evaluator", repo,
                                          repository.common_dir(repo), roots, repo, ROW_ENV),
                             cage.Policy("default"), 0, HOME)
    assert cage.RUN_ROW_TOOL in json.loads(evaluator.settings.read_text())["permissions"]["allow"]
    assert json.loads(evaluator.policy.read_text())["run_row"] == {"cwd": str(repo), "env": ROW_ENV}
    builder = _prepared(repo)
    assert cage.RUN_ROW_TOOL not in json.loads(builder.settings.read_text())["permissions"]["allow"]
    assert "run_row" not in json.loads(builder.policy.read_text())


def test_run_row_is_listed_only_when_the_policy_has_it(repo, tmp_path):
    listing = {"jsonrpc": "2.0", "id": 1, "method": "tools/list"}
    plain = _session(_policy(repo, tmp_path), listing)[0]["result"]["tools"]
    served = _session(_policy(repo, tmp_path, role="evaluator", tools=["run_row"], run_row={"cwd": str(repo), "env": ROW_ENV}), listing)[0]["result"]["tools"]
    assert "run_row" not in {tool["name"] for tool in plain}
    assert "run_row" in {tool["name"] for tool in served}


@pytest.mark.parametrize("row_id, viewport", [("../x", ""), ("R1; id", ""), ("", ""), ("R1", "wide")])
def test_run_row_refuses_anything_but_a_row_id_and_a_viewport(repo, tmp_path, row_id, viewport):
    policy = _policy(repo, tmp_path, role="evaluator", tools=["run_row"], run_row={"cwd": str(repo), "env": ROW_ENV})
    assert "not a" in cage_server.run_row(policy, row_id, viewport)


def test_run_row_does_nothing_for_a_role_without_the_tool(repo, tmp_path):
    assert "not available" in cage_server.run_row(_policy(repo, tmp_path), "R1")


def test_collect_reads_the_request_log_and_skips_junk(repo):
    run_cage = _prepared(repo)
    run_cage.requests.write_text('{"tool": "WebFetch", "detail": "x", "reason": "r", "run": "build-1"}\nnot json\n')
    assert cage.collect(run_cage) == [{"at": "", "run": "build-1", "role": "", "tool": "WebFetch", "detail": "x",
                                       "reason": "r"}]


# -- the engine's commit for Codex roles -----------------------------------------------------------


def _worktree(repo, tmp_path):
    path = tmp_path / "lane"
    git(repo, "worktree", "add", "-q", "-b", "lane", str(path))
    return path


def test_claude_codes_own_sandbox_record_does_not_trip_the_guard(repo):
    from sakura import guard

    (repo / ".claude").mkdir()
    (repo / ".claude" / "settings.json").write_text("{}")
    before = guard.fingerprint(repo)
    (repo / ".claude" / ".cc-writes").write_text("record")
    assert guard.fingerprint(repo) == before
    (repo / ".claude" / "settings.local.json").write_text("{}")
    assert guard.fingerprint(repo) != before


def test_claude_codes_sandbox_creating_the_claude_folder_itself_does_not_trip_the_guard(repo):
    from sakura import guard

    before = guard.fingerprint(repo)
    (repo / ".claude" / ".cc-writes").mkdir(parents=True)
    assert guard.fingerprint(repo) == before
    (repo / ".claude" / "settings.json").write_text("{}")
    assert guard.fingerprint(repo) != before


def test_seatbelt_paths_are_real_paths_and_git_commits_work_in_a_worktree(repo, tmp_path):
    lane = tmp_path / "lane"
    git(repo, "worktree", "add", "-q", "-b", "lane", str(lane))
    run_cage = cage.prepare(_site(lane), cage.Policy("default"), 2, Path.home())
    assert "(subpath \"/var/" not in run_cage.seatbelt.read_text()
    assert str(run_cage.tmp).startswith("/private/")
    done = _caged(run_cage, "echo x > work.txt && git add work.txt && git -c core.hooksPath=/dev/null commit -qm caged",
                  lane)
    assert done.returncode == 0, done.stderr


def test_caged_commits_never_sign_with_the_persons_key(repo, tmp_path):
    keys = tmp_path / "keys"
    keys.mkdir()
    (keys / "id.pub").write_text("ssh-ed25519 AAAA test\n")
    for key, value in (("commit.gpgsign", "true"), ("gpg.format", "ssh"), ("user.signingkey", str(keys / "id.pub"))):
        git(repo, "config", key, value)
    run_cage = _prepared(repo, policy=cage.Policy("keys", secret_paths=(str(keys),)), home=Path.home())
    done = _caged(run_cage, "echo x > w.txt && git add w.txt && git -c core.hooksPath=/dev/null commit -qm caged", repo)
    assert done.returncode == 0, done.stderr
    assert git(repo, "log", "-1", "--format=%G?") == "N"


def test_nested_env_files_are_denied_to_the_file_tools_anchored_at_each_root(repo):
    deny = json.loads(_prepared(repo).settings.read_text())["permissions"]["deny"]
    assert "Read(./**/.env*)" in deny and "Edit(./**/.env*)" in deny
    assert f"Read(/{repo}/**/.env*)" in deny and f"Read(/{HOME}/**/.env*)" in deny
    policy = cage.Policy("glob", secret_paths=("~/vault/**/*.key",))
    deny = json.loads(_prepared(repo, policy=policy).settings.read_text())["permissions"]["deny"]
    assert f"Read(/{HOME}/vault/**/*.key)" in deny and f"Edit(/{HOME}/vault/**/*.key)" in deny


# -- reads in the home are default-deny --------------------------------------------------------------


def test_claude_settings_deny_the_home_and_allow_back_only_what_a_lane_needs(repo):
    filesystem = json.loads(_prepared(repo).settings.read_text())["sandbox"]["filesystem"]
    roots = len(cage.TOP_LEVEL_READ_DENIALS)
    assert filesystem["denyRead"][:roots] == list(cage.TOP_LEVEL_READ_DENIALS) and filesystem["denyRead"][roots] == str(HOME)
    allowed = filesystem["allowRead"]
    assert str(repo) in allowed and all(str(KIT_BIN.parents[1] / name) in allowed for name in cage.ENGINE_RUNTIME_FOLDERS)
    assert str(HOME / ".cargo" / "bin") in allowed and str(HOME / ".rustup") in allowed and str(HOME / ".npm") in allowed
    assert not any(path in allowed for path in (str(HOME), str(HOME / ".ssh"), str(HOME / ".config")))


def test_seatbelt_reads_in_the_home_are_default_deny(tmp_path):
    home = tmp_path / "home"
    (home / "fresh").mkdir(parents=True)
    (home / "fresh" / "canary.txt").write_text("canary")
    (home / ".cargo" / "bin").mkdir(parents=True)
    (home / ".cargo" / "bin" / "tool").write_text("toolchain")
    (home / ".ssh").mkdir()
    (home / ".ssh" / "id").write_text("key")
    lane = home / "development" / "lane"
    lane.parent.mkdir(parents=True)
    git(lane.parent, "init", "-q", str(lane))
    (lane / ".env").write_text("TOKEN=1")
    (lane / "app.py").write_text("print('app')\n")
    site = cage.RunSite(engine_home.root() / "proj" / "flow" / "demo", "build-1", "builder", lane,
                        repository.common_dir(lane), (engine_home.root(),), lane)
    policy = cage.Policy("extras", read_paths=("~/extra",))
    (home / "extra").mkdir()
    (home / "extra" / "data").write_text("extra")
    run_cage = cage.prepare(site, policy, 2, home)
    blocked = {"fresh home folder": f"cat {home / 'fresh' / 'canary.txt'}", "ssh": f"cat {home / '.ssh' / 'id'}",
               "env file in the workspace": "cat .env", "home listing": f"ls {home}"}
    for name, command in blocked.items():
        assert _caged(run_cage, command, lane).returncode != 0, name
    allowed = {"workspace": "cat app.py", "toolchain": f"cat {home / '.cargo' / 'bin' / 'tool'}",
               "policy extra": f"cat {home / 'extra' / 'data'}", "sakura": f"{KIT_BIN} --help > /dev/null",
               "git": "git status --short"}
    for name, command in allowed.items():
        done = _caged(run_cage, command, lane)
        assert done.returncode == 0, (name, done.stdout, done.stderr)


def test_caged_git_needs_no_global_config_and_commits_as_the_person(repo):
    env = _prepared(repo).env
    assert env["GIT_CONFIG_GLOBAL"] == "/dev/null"
    assert env["GIT_AUTHOR_NAME"] == "Test User" and env["GIT_COMMITTER_EMAIL"] == "test@example.invalid"


@pytest.mark.parametrize("read_path", ["~/", "/", "~/.config", "~/.ssh"])
def test_read_paths_may_not_cover_the_home_a_secret_or_an_engine_home(repo, read_path):
    with pytest.raises(cage.CageError, match="stays denied"):
        _prepared(repo, policy=cage.Policy("wide", read_paths=(read_path,)))
    with pytest.raises(cage.CageError, match="stays denied"):
        _prepared(repo, policy=cage.Policy("engine", read_paths=(str(engine_home.root().parent),)))


def test_a_secret_inside_an_allowed_tree_cannot_be_read_renamed_or_linked(repo, tmp_path):
    # The workspace and the default toolchain folders are allowed trees that may hold a secret: the deny wins.
    home = tmp_path / "home"
    tree = home / ".rustup"
    tree.mkdir(parents=True)
    (tree / "secret.txt").write_text("secret")
    (tree / "public.txt").write_text("public")
    (repo / ".env").write_text("TOKEN=1")
    policy = cage.Policy("tree", secret_paths=("~/.rustup/secret.txt",))
    run_cage = cage.prepare(_site(repo), policy, 2, home)
    assert _caged(run_cage, f"cat {tree / 'public.txt'}", repo).returncode == 0
    for command in (f"cat {tree / 'secret.txt'}", "cat .env", "mv .env moved && cat moved", "ln .env linked && cat linked",
                    "cp .env copied && cat copied", "grep -r TOKEN ."):
        done = _caged(run_cage, command, repo)
        assert "TOKEN=1" not in done.stdout and "secret" not in done.stdout.replace("secret.txt", ""), command
    assert (repo / ".env").read_text() == "TOKEN=1"


def test_one_secret_list_feeds_every_enforcement_point(repo):
    policy = cage.Policy("sync", secret_paths=("~/.vault", "/opt/keys"))
    run_cage = _prepared(repo, policy=policy)
    settings = json.loads(run_cage.settings.read_text())
    profile = run_cage.seatbelt.read_text().splitlines()
    read_deny = next(line for line in profile if line.startswith("(deny file-read*") and ".vault" in line)
    write_deny = next(line for line in profile if line.startswith("(deny file-write*") and ".vault" in line)
    for secret in cage.secret_paths(policy, HOME, repo):
        assert secret in settings["sandbox"]["filesystem"]["denyRead"], secret
        assert secret in settings["sandbox"]["filesystem"]["denyWrite"], secret
        assert f"Read(/{secret})" in settings["permissions"]["deny"], secret
        assert f"Edit(/{secret})" in settings["permissions"]["deny"], secret
        prefix = os.path.realpath(secret.partition("/**/")[0])
        fragment = re.escape(prefix) if "**/" in secret else f'(subpath "{prefix}")'
        assert fragment in read_deny and fragment in write_deny, secret


def test_every_agent_account_folder_is_a_secret(repo, tmp_path):
    home = tmp_path / "home"
    (home / ".claude-work" / "projects").mkdir(parents=True)
    (home / ".codex-work" / "sessions").mkdir(parents=True)
    secrets = cage.secret_paths(cage.Policy("default"), home, repo)
    assert os.path.realpath(home / ".claude-work") in secrets and os.path.realpath(home / ".codex-work") in secrets


# -- account login folders: a static floor, the registry (fail closed) and name patterns -------------------


def _registry(home, text):
    path = home / ".config" / "sakura" / "accounts.toml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def test_account_folders_keep_a_static_floor_in_an_empty_home(repo, tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    secrets = cage.secret_paths(cage.Policy("default"), home, repo)
    for name in (".claude", ".claude-work", ".codex"):
        assert os.path.realpath(home / name) in secrets or str(home / name) in secrets, name


def test_registered_account_folders_are_secrets_even_when_disabled(repo, tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    login = tmp_path / "elsewhere" / "agent-login"
    _registry(home, f'[[accounts]]\nprovider = "claude"\nid = "side"\nconfig_dir = "{login}"\nenabled = false\n')
    assert os.path.realpath(login) in cage.secret_paths(cage.Policy("default"), home, repo)


def test_an_unreadable_account_registry_fails_closed(repo, tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    _registry(home, "[[accounts]\nbroken")
    with pytest.raises(cage.CageError, match="account"):
        cage.secret_paths(cage.Policy("default"), home, repo)


def test_folders_named_like_claude_or_codex_count_without_transcripts(repo, tmp_path):
    home = tmp_path / "home"
    for name in (".claude-work", ".my-codex", ".notes"):
        (home / name).mkdir(parents=True)
    secrets = cage.secret_paths(cage.Policy("default"), home, repo)
    assert os.path.realpath(home / ".claude-work") in secrets and os.path.realpath(home / ".my-codex") in secrets
    assert os.path.realpath(home / ".notes") not in secrets


# -- the Codex reviewer profile, measured with the installed Codex ------------------------------------


def _codex_sandbox(repo, command):
    """Run `command` under the permission profile the engine generates for a Codex reviewer; returns the result."""
    import shutil

    if shutil.which("codex") is None:
        pytest.skip("codex is not installed")
    run_cage = _prepared(repo, role="code_reviewer", home=Path(os.path.realpath(Path.home())))
    args = list(run_cage.codex_args)
    return subprocess.run(["codex", "sandbox", "-P", cage.CODEX_PROFILE, *args[2:], "-C", str(repo), "--", "sh", "-c", command],
                          capture_output=True, text=True, stdin=subprocess.DEVNULL, timeout=60)


def test_codex_profile_denies_top_level_nested_and_late_env_files_and_all_writes(repo):
    (repo / "sub").mkdir()
    for name in (".env", "sub/.env"):
        (repo / name).write_text("env-secret\n")
    (repo / "plain.txt").write_text("plain\n")
    done = _codex_sandbox(repo, "cat .env; cat sub/.env; cat plain.txt; echo w > new.txt; mkdir sub/deep")
    assert "env-secret" not in done.stdout and done.stdout.count("plain") == 1
    assert done.stderr.count("Operation not permitted") >= 4
    assert not (repo / "new.txt").exists()
    # A `.env` file that appears after the run began is covered too: the deny is a path pattern, not a file list.
    import threading

    threading.Timer(1.0, lambda: (repo / "sub" / ".env.late").write_text("late-secret\n")).start()
    late = _codex_sandbox(repo, "sleep 3; cat sub/.env.late")
    assert "late-secret" not in late.stdout and "Operation not permitted" in late.stderr


def test_CodexProfile_KeepsThePasteboardKeychainAndLaunchServicesOutOfReach(repo):
    # Codex writes its own Seatbelt profile from ours, which carries files and network only. Measured on 0.164.0:
    # these services are denied, but another process's environment is still readable (canary table, review 3).
    (repo / "bootstrap.py").write_text(BOOTSTRAP)
    done = _codex_sandbox(repo, "pbpaste > /dev/null 2>&1; echo pbpaste=$?; "
                                "security find-generic-password > /dev/null 2>&1; echo security=$?; "
                                f"{sys.executable} -I bootstrap.py {' '.join(SENSITIVE_SERVICES)}")
    lines = dict(line.rsplit("=", 1) if "=" in line else line.rsplit(" ", 1) for line in done.stdout.splitlines())
    assert lines.get("pbpaste") not in (None, "0") and lines.get("security") not in (None, "0"), done.stdout + done.stderr
    assert all(lines.get(name) not in (None, "0") for name in SENSITIVE_SERVICES), done.stdout + done.stderr


def test_Cage_GitdirFilesOfTheCommonFolder_AreNotWritableButTheWorktreeStateIs(repo, tmp_path):
    lane = tmp_path / "lane"
    git(repo, "worktree", "add", "-q", "-b", "lane", str(lane))
    common = repository.common_dir(repo)
    entry = common / "worktrees" / "lane"
    (common / "info").mkdir(exist_ok=True)
    run_cage = cage.prepare(_site(lane), cage.Policy("default"), 2, Path.home())
    probes = {"worktree config": entry / "config.worktree", "gitdir pointer": entry / "gitdir",
              "commondir pointer": entry / "commondir", "attributes": common / "info" / "attributes",
              "main worktree config": common / "config.worktree"}
    for name, path in probes.items():
        assert _caged(run_cage, f"echo x > {path}", lane).returncode != 0, name
        assert not path.exists() or path.read_text() != "x\n", name
    deny = json.loads(run_cage.settings.read_text())["sandbox"]["filesystem"]["denyWrite"]
    assert str(common / "worktrees" / "**" / "config.worktree") in deny
    done = _caged(run_cage, "echo x > work.txt && git add work.txt && git -c core.hooksPath=/dev/null commit -qm caged", lane)
    assert done.returncode == 0, done.stderr


# -- review 4: reads outside the home ---------------------------------------------------------------------------


@pytest.fixture
def outside_canaries():
    """Canary files in the shared folders outside the home, each in a folder of its own; removed afterwards."""
    import shutil
    import tempfile

    folders = [Path(tempfile.mkdtemp(prefix="sakura-canary-", dir=root))
               for root in ("/Users/Shared", "/private/tmp", os.path.realpath(tempfile.gettempdir()))]
    for folder in folders:
        (folder / "canary.txt").write_text("outside-canary\n")
    yield folders
    for folder in folders:
        shutil.rmtree(folder, ignore_errors=True)


def test_Seatbelt_ReadsOfSharedFoldersOtherHomesVolumesAndTemp_AreDenied(repo, outside_canaries):
    run_cage = _prepared(repo, home=Path.home())
    for folder in outside_canaries:
        done = _caged(run_cage, f"cat {folder / 'canary.txt'}", repo)
        assert done.returncode != 0 and "outside-canary" not in done.stdout, folder
    for listing in ("/Users", "/Volumes", "/private/tmp", "/private/var/folders"):
        assert _caged(run_cage, f"ls {listing}", repo).returncode != 0, listing


def test_Seatbelt_TheRunAndWorkspaceAreReadableWithPinnedToolchainRoots(repo):
    run_cage = _prepared(repo)
    allows = [line for line in run_cage.seatbelt.read_text().splitlines() if line.startswith("(allow file-read* ")]
    for folder in (str(repo.resolve()), str(run_cage.tmp.resolve()), "/usr", "/bin"):
        assert any(f'"{folder}"' in line for line in allows), folder


@seatbelt_integration
def test_Seatbelt_TheRunsOwnTempTheWorkspaceAndTheToolchainsStayUsable(repo):
    run_cage = _prepared(repo, home=Path.home())
    (repo / "app.py").write_text("print('app')\n")
    commands = {"run tmp": 'echo t > "$TMPDIR/t" && cat "$TMPDIR/t"', "workspace": "cat app.py",
                "python": f"{sys.executable} -c 'import json, ssl, sqlite3, tempfile; tempfile.mkstemp()'",
                "git": "git status --short", "system git": "/usr/bin/git --version", "sakura": f"{KIT_BIN} --help"}
    # cargo is left to the canary: its rustup proxy needs the real home's .rustup, and tests run on an isolated home.
    for tool in ("node", "npm"):
        found = shutil.which(tool, path="/opt/homebrew/bin:/usr/local/bin:/usr/bin")
        if found:
            commands[tool] = f"{found} --version"
    for name, command in commands.items():
        done = _caged(run_cage, command, repo)
        assert done.returncode == 0, (name, done.stdout[-500:], done.stderr[-500:])


def test_ClaudeSettings_DenyReadsOfSharedFoldersOtherHomesVolumesAndTempButTheRunsOwn(repo):
    run_cage = _prepared(repo)
    filesystem = json.loads(run_cage.settings.read_text())["sandbox"]["filesystem"]
    assert {"/Users", "/Volumes", "/private/tmp", "/private/var/folders"} <= set(filesystem["denyRead"])
    assert str(run_cage.tmp) in filesystem["allowRead"]
    assert not any(path in filesystem["allowRead"] for path in ("/private/tmp", "/private/var/folders", "/Users"))


def test_Seatbelt_AnExecRolesOwnProgram_StaysReadableWhileItsNeighboursDoNot(repo, outside_canaries):
    script = outside_canaries[2] / "agent.py"
    script.write_text("print('agent ran')\n")
    site = cage.RunSite(engine_home.root() / "proj" / "flow" / "demo", "plan-1", "planner", repo,
                        repository.common_dir(repo), (engine_home.root(),), repo, program_paths=(str(script),))
    run_cage = cage.prepare(site, cage.Policy("default"), 0, Path.home())
    assert _caged(run_cage, f"{sys.executable} {script}", repo).stdout == "agent ran\n"
    assert _caged(run_cage, f"cat {outside_canaries[2] / 'canary.txt'}", repo).returncode != 0


# -- review 4, second pass: firmlink aliases and the remaining shared folders -----------------------------------


DATA_VOLUME = "/System/Volumes/Data"


@pytest.mark.skipif(not Path(DATA_VOLUME).is_dir(), reason="needs the APFS data volume")
def test_Seatbelt_ReadsThroughTheDataVolumeAliasOfADeniedPath_AreDenied(repo, tmp_path, outside_canaries):
    home = tmp_path / "home"
    (home / "fresh").mkdir(parents=True)
    (home / "fresh" / "canary.txt").write_text("outside-canary\n")
    secret = tmp_path / "secret"
    secret.mkdir()
    (secret / "s.txt").write_text("outside-canary\n")
    run_cage = cage.prepare(_site(repo), cage.Policy("canary", secret_paths=(str(secret),)), 2, home)
    targets = [home / "fresh" / "canary.txt", secret / "s.txt", *(folder / "canary.txt" for folder in outside_canaries)]
    for target in targets:
        alias = DATA_VOLUME + os.path.realpath(target)
        assert Path(alias).read_text() == "outside-canary\n", "the alias reaches the same file outside the cage"
        done = _caged(run_cage, f"cat {alias}", repo)
        assert done.returncode != 0 and "outside-canary" not in done.stdout, alias
    assert _caged(run_cage, "cat app.py" if (repo / "app.py").exists() else "cat README", repo).returncode == 0


def test_Seatbelt_ReadsOfVarTmpAndCores_AreDenied(repo):
    run_cage = _prepared(repo, home=Path.home())
    for folder in ("/private/var/tmp", "/cores"):
        if Path(folder).is_dir():
            assert _caged(run_cage, f"ls {folder}", repo).returncode != 0, folder


def test_ClaudeSettingsAndCodex_DenyTheDataVolumeAliasOfEveryDeniedPath(repo):
    run_cage = _prepared(repo)
    deny = json.loads(run_cage.settings.read_text())["sandbox"]["filesystem"]["denyRead"]
    assert {"/private/var/tmp", "/cores"} <= set(deny)
    for path in (str(HOME), "/Users", "/private/tmp", "/private/var/folders", "/Library/Keychains"):
        assert DATA_VOLUME + path in deny, path
    profile = next(arg for arg in run_cage.codex_args if arg.startswith("permissions.sakura-") and ".filesystem=" in arg)
    assert f'"{DATA_VOLUME}/Users" = "none"' in profile


def test_ClaudeLane_ItsOwnTempRootIsTheRunsTmpFolder_NotTheSharedOne(repo):
    run_cage = _prepared(repo)
    own = Path(run_cage.env["CLAUDE_CODE_TMPDIR"])
    assert own.is_dir() and own.parent == run_cage.tmp and (own.stat().st_mode & 0o777) == 0o700
    filesystem = json.loads(run_cage.settings.read_text())["sandbox"]["filesystem"]
    assert str(run_cage.tmp) in filesystem["allowRead"] and "/private/tmp" in filesystem["denyRead"]


def _git_settings(env):
    return {env[f"GIT_CONFIG_KEY_{index}"]: env[f"GIT_CONFIG_VALUE_{index}"] for index in range(int(env["GIT_CONFIG_COUNT"]))}


def test_a_caged_command_gets_the_workspace_hooks_through_the_environment_not_the_repository_config(repo):
    (repo / ".sakura/git-hooks").mkdir(parents=True)

    settings = _git_settings(_prepared(repo).env)

    assert settings["core.hooksPath"] == str(repo / ".sakura/git-hooks")
    assert settings["commit.gpgsign"] == "false" and settings["tag.gpgsign"] == "false"


def test_a_workspace_without_installed_hooks_gets_no_hooks_path(repo):
    assert "core.hooksPath" not in _git_settings(_prepared(repo).env)


# -- credential-bearing system folders: denied in every cage, their data-volume names too -----------------------------

CREDENTIAL_FOLDERS = ("/opt/homebrew/etc", "/opt/homebrew/var", "/usr/local/etc", "/usr/local/var", "/private/etc/ssh",
                      "/private/var/db", "/private/var/log", "/Library/Logs", "/Library/Preferences/SystemConfiguration")


@pytest.fixture
def homebrew_canary():
    """A canary file in Homebrew's `etc`, where service configs with passwords live; skipped when it is not writable."""
    folder = Path("/opt/homebrew/etc")
    if not os.access(folder, os.W_OK):
        pytest.skip("/opt/homebrew/etc is not writable here")
    canary = folder / "sakura-canary-9d2e.conf"
    canary.write_text("system-canary\n")
    yield canary
    canary.unlink(missing_ok=True)


def test_Cage_EverySettingsFileAndProfileDeniesTheCredentialBearingSystemFolders(repo):
    run_cage = _prepared(repo)
    deny = json.loads(run_cage.settings.read_text())["sandbox"]["filesystem"]["denyRead"]
    seatbelt = run_cage.seatbelt.read_text()
    codex = next(arg for arg in run_cage.codex_args if arg.startswith("permissions.sakura-") and ".filesystem=" in arg)

    for folder in CREDENTIAL_FOLDERS:
        assert folder in deny and DATA_VOLUME + folder in deny, folder
        assert f'"{folder}"' in seatbelt, folder
        assert f'"{folder}" = "none"' in codex and f'"{DATA_VOLUME}{folder}" = "none"' in codex, folder
    assert "/private/var/db/timezone" in json.loads(run_cage.settings.read_text())["sandbox"]["filesystem"]["allowRead"]


def test_Seatbelt_ACanaryInHomebrewEtcAndTheSystemsSshFolder_AreUnreadable(repo, homebrew_canary):
    run_cage = _prepared(repo, home=Path.home())

    assert _caged(run_cage, f"cat {homebrew_canary}", repo).returncode != 0
    assert "system-canary" not in _caged(run_cage, f"cat {homebrew_canary}", repo).stdout
    for readable_on_this_mac in ("/private/etc/ssh/ssh_config", "/private/var/db/.AppleSetupDone"):
        if Path(readable_on_this_mac).exists():
            assert _caged(run_cage, f"cat {readable_on_this_mac}", repo).returncode != 0, readable_on_this_mac


def test_Seatbelt_TheTimezoneExceptionHasTheLastMatchingReadRule(repo):
    lines = _prepared(repo).seatbelt.read_text().splitlines()
    zone = [index for index, line in enumerate(lines) if line.startswith("(allow file-read* ")
            and '"/private/var/db/timezone"' in line]
    denied = [index for index, line in enumerate(lines) if line.startswith("(deny file-read* ")
              and '"/private/var/db"' in line]
    assert zone and denied and max(zone) > max(denied)


@seatbelt_integration
def test_Seatbelt_TheTimeZoneStaysReadableWhileTheRestOfVarDbIsDenied(repo):
    run_cage = _prepared(repo, home=Path.home())
    assert _caged(run_cage, "/bin/date +%z", repo, {"TZ": "Asia/Kathmandu"}).stdout.strip() == "+0545"
    assert _caged(run_cage, "ls /private/var/db", repo).returncode != 0


def test_Seatbelt_GitAndTheCertificateBundlesStillWorkWhileHomebrewEtcIsDenied(repo):
    """Homebrew's git reads its system config from `etc`, and its OpenSSL reads the CA bundle there."""
    run_cage = _prepared(repo, home=Path.home())

    assert _caged(run_cage, "git --version", repo).returncode == 0
    assert _caged(run_cage, "git status --short", repo).returncode == 0
    for allowed in ("/opt/homebrew/etc/gitconfig", "/opt/homebrew/etc/ca-certificates/cert.pem"):
        if Path(allowed).exists():
            assert _caged(run_cage, f"cat {allowed} > /dev/null", repo).returncode == 0, allowed


def test_ClaudeSettings_AllowBackOnlyTheNonSecretFilesOfHomebrewEtc(repo):
    allow = json.loads(_prepared(repo).settings.read_text())["sandbox"]["filesystem"]["allowRead"]

    assert {"/opt/homebrew/etc/gitconfig", "/opt/homebrew/etc/ca-certificates"} <= set(allow)
    assert "/opt/homebrew/etc" not in allow and not any(path.endswith("/private") for path in allow)


# -- devices (review 6, finding 1) --------------------------------------------------------------------


@pytest.fixture
def other_terminal():
    """A pseudo-terminal of this user that is not the caged command's own: its slave is a `/dev/ttys*` device."""
    import pty

    master, slave = pty.openpty()
    try:
        yield os.ttyname(slave)
    finally:
        os.close(master)
        os.close(slave)


def test_Seatbelt_AnotherTerminalDevice_CannotBeOpenedForWriting(repo, other_terminal):
    run_cage = _prepared(repo, home=Path.home())
    done = _caged(run_cage, f"echo injected > {other_terminal}", repo)
    assert done.returncode != 0 and "Operation not permitted" in done.stderr, (done.stdout, done.stderr)


@pytest.mark.parametrize("build_dir", [False, True])
def test_Seatbelt_OnlyTheNamedDevicesAreWritable(repo, build_dir):
    site = _site(repo)
    site = cage.RunSite(*(getattr(site, name) for name in ("task_root", "run", "role", "workspace", "common_dir",
                                                           "engine_roots", "repo")),
                        build_dir=cage.new_build_dir() if build_dir else None)
    try:
        run_cage = cage.prepare(site, cage.Policy("default"), 2, Path.home())
        for device in ("/dev/null", "/dev/zero", "/dev/random", "/dev/urandom"):
            done = _caged(run_cage, f"head -c 4 {device} > /dev/null; echo x > /dev/null", repo)
            assert done.returncode == 0, (device, done.stderr)
        assert _caged(run_cage, "echo ok > /dev/fd/1", repo).stdout.strip() == "ok"
        assert _caged(run_cage, "echo ok > /dev/stdout", repo).stdout.strip() == "ok"
        for device in ("/dev/tty", "/dev/console", "/dev/ptmx", "/dev/disk0", "/dev/rdisk0", "/dev/dtracehelper"):
            done = _caged(run_cage, f"echo x > {device}", repo)
            assert done.returncode != 0 and "Operation not permitted" in done.stderr, (device, done.stderr)
    finally:
        if site.build_dir:
            import shutil

            shutil.rmtree(site.build_dir, ignore_errors=True)


def test_Seatbelt_ProfileNamesNoWholeDeviceFolder(repo):
    profile = _prepared(repo, home=Path.home()).seatbelt.read_text()
    assert '(subpath "/dev")' not in profile
    for device in cage.WRITABLE_DEVICES:
        assert f'(literal "{device}")' in profile


# -- reads are default-deny (review 6, finding 4) ------------------------------------------------------

UNLISTED_FOLDERS = ("/Library/Application Support", "/Library/Preferences", "/Applications", "/opt", "/opt/homebrew/Caskroom",
                    "/private/etc", "/private/var/tmp", "/private/var/root", "/System/Volumes/Data/Library/Application Support",
                    "/System/Volumes/Data/opt")


@pytest.mark.parametrize("folder", [path for path in UNLISTED_FOLDERS if os.path.isdir(path)])
def test_Seatbelt_AFolderNoRuleNames_CannotBeListed(repo, folder):
    run_cage = _prepared(repo, home=Path.home())
    done = _caged(run_cage, f'ls "{folder}"', repo)
    assert done.returncode != 0 and not done.stdout.strip(), (folder, done.stdout[:200], done.stderr[:200])


def test_Seatbelt_ACredentialFileOutsideTheListedTrees_CannotBeRead(repo):
    run_cage = _prepared(repo, home=Path.home())
    for path in ("/private/etc/pam.d/sudo", "/private/etc/sudoers", "/private/etc/ssh/ssh_config", "/private/etc/zshrc"):
        if os.access(path, os.R_OK):
            done = _caged(run_cage, f'cat "{path}"', repo)
            assert done.returncode != 0 and not done.stdout, path


def test_Seatbelt_ProfileDeniesEveryReadFirstAndNamesWhatComesBack(repo):
    lines = _prepared(repo, home=Path.home()).seatbelt.read_text().splitlines()
    assert lines.index("(deny file-read*)") < min(index for index, line in enumerate(lines) if line.startswith("(allow file-read*"))
    allowed = next(line for line in lines if line.startswith("(allow file-read*") and '"/System"' in line)
    for tree in ("/usr", "/bin", "/sbin", "/Library/Developer", "/opt/homebrew/bin", "/opt/homebrew/Cellar"):
        assert f'(subpath "{tree}")' in allowed, tree
    for tree in ("/opt/homebrew/etc", "/opt/homebrew/var", "/private/etc/ssh", "/Library/Application Support"):
        assert f'(subpath "{tree}")' not in allowed, tree


def test_Seatbelt_TheSystemsToolsStillRunUnderDefaultDeny(repo):
    run_cage = _prepared(repo, home=Path.home())
    commands = {"date": "date", "curl": "curl --version", "system git": "/usr/bin/git --version", "ls": "ls /usr/bin | head -1",
                "sh": "/bin/sh -c true", "python": f"{sys.executable} -c 'import ssl, sqlite3, zoneinfo; print(1)'"}
    for name, command in commands.items():
        done = _caged(run_cage, command, repo)
        assert done.returncode == 0, (name, done.stdout[-300:], done.stderr[-300:])


def test_CodexProfile_AnotherTerminalDeviceCannotBeWrittenEvenThoughTheCodexDefaultsAllowIt(repo, other_terminal):
    done = _codex_sandbox(repo, f"echo injected > {other_terminal}; echo rc=$?; echo ok > /dev/null; echo null=$?")
    assert "rc=1" in done.stdout and "null=0" in done.stdout and "Operation not permitted" in done.stderr, done.stdout + done.stderr


@pytest.mark.parametrize("command", ["ls /Applications", "ls /private/etc/ssh", "cat /private/etc/pam.d/sudo",
                                     "cat /private/var/db/.AppleSetupDone", "ls /Library/Preferences", "ls /opt/homebrew/Caskroom"])
def test_CodexProfile_TheFoldersCodexsDefaultsWouldReadButAReviewerDoesNotNeed_AreDenied(repo, command):
    if not os.path.exists(command.split()[-1]):
        pytest.skip(f"{command.split()[-1]} does not exist here")
    done = _codex_sandbox(repo, command)
    assert "Operation not permitted" in done.stderr and not done.stdout.strip(), (command, done.stdout[:200], done.stderr[:200])


def test_CodexProfile_TheReviewersToolsStillRunOnTheExplicitRoots(repo):
    (repo / "app.py").write_text("print('app')\n")
    done = _codex_sandbox(repo, f"cat app.py; {sys.executable} -c 'import json, ssl'; echo python=$?; ls /usr/bin | head -1")
    assert "print('app')" in done.stdout and "python=0" in done.stdout, done.stdout + done.stderr


def test_ClaudeSettings_DenyEveryTopLevelFolderButTheSystemsAndAllowBackTheNamedTrees(repo):
    filesystem = json.loads(_prepared(repo).settings.read_text())["sandbox"]["filesystem"]
    denied, allowed = set(filesystem["denyRead"]), filesystem["allowRead"]
    for folder in cage.TOP_LEVEL_READ_DENIALS:
        assert folder in denied and cage.DATA_VOLUME + folder in denied, folder
    assert not denied & {"/", "/System", "/usr", "/bin", "/sbin", "/dev"}
    for tree in ("/System", "/usr", "/bin", "/sbin", "/Library/Developer", "/opt/homebrew/bin", "/opt/homebrew/Cellar"):
        assert tree in allowed, tree
    # A `/` entry would open the whole volume again; the credential-bearing folders stay denied and out of the list.
    assert "/" not in allowed and not {"/opt/homebrew/etc", "/opt/homebrew/var", "/private/etc/ssh"} & set(allowed)
    assert {"/opt/homebrew/etc", "/opt/homebrew/var", "/private/etc/ssh"} <= denied


# -- the interpreter grant is narrow (coordinator finding on the read allow-list) -------------------------------------


def _interpreter(monkeypatch, prefix):
    """Make `prefix` the engine's interpreter tree, with the folders a Python install has."""
    for name in cage.INTERPRETER_FOLDERS:
        (prefix / name).mkdir(parents=True, exist_ok=True)
    monkeypatch.setattr(sys, "base_prefix", str(prefix))


def test_Interpreter_OutsideTheHome_IsGrantedAsItsThreeFoldersNeverTheWholePrefix(tmp_path, monkeypatch):
    prefix, home = tmp_path / "opt" / "py", tmp_path / "home"
    home.mkdir()
    _interpreter(monkeypatch, prefix)
    granted = cage._interpreter_trees(home, [], ())
    assert sorted(granted) == sorted(str(Path(os.path.realpath(prefix / name))) for name in cage.INTERPRETER_FOLDERS)
    assert str(Path(os.path.realpath(prefix))) not in granted


def test_Interpreter_ThePrefixIsTheHome_OrAFolderIsTheHome_NothingIsGranted(tmp_path, monkeypatch):
    home = tmp_path / "home"
    _interpreter(monkeypatch, home)
    assert cage._interpreter_trees(home, [], ()) == []
    prefix, bin_home = tmp_path / "py", tmp_path / "py" / "bin"
    _interpreter(monkeypatch, prefix)
    granted = cage._interpreter_trees(bin_home, [], ())
    assert str(Path(os.path.realpath(bin_home))) not in granted and len(granted) == len(cage.INTERPRETER_FOLDERS) - 1


def test_Interpreter_AFolderThatHoldsTheHome_IsLeftOut(tmp_path, monkeypatch):
    prefix = tmp_path / "py"
    _interpreter(monkeypatch, prefix)
    home = prefix / "lib" / f"python{sys.version_info.major}.{sys.version_info.minor}" / "home"
    home.mkdir()
    granted = cage._interpreter_trees(home, [], ())
    assert not any(Path(path) in (home, *home.parents) for path in granted) and granted


def test_Interpreter_UnderTheHome_OnlyAKnownPerVersionFolderIsAccepted(tmp_path, monkeypatch):
    home = tmp_path / "home"
    _interpreter(monkeypatch, home / "somewhere" / "py")
    assert cage._interpreter_trees(home, [], ()) == []
    _interpreter(monkeypatch, home / ".pyenv" / "versions" / "3.99.0")
    assert len(cage._interpreter_trees(home, [], ())) == len(cage.INTERPRETER_FOLDERS)
    _interpreter(monkeypatch, home / ".local" / "share" / "uv" / "python" / "cpython-3.99")
    assert len(cage._interpreter_trees(home, [], ())) == len(cage.INTERPRETER_FOLDERS)


def test_Interpreter_AFolderHoldingOrInsideASecretOrAnEngineHome_IsLeftOut(tmp_path, monkeypatch):
    home = tmp_path / "home"
    prefix = home / ".pyenv" / "versions" / "3.99.0"
    _interpreter(monkeypatch, prefix)
    library = prefix / "lib" / f"python{sys.version_info.major}.{sys.version_info.minor}"
    secret = library / "site-packages" / "planted-secret"
    engine = prefix / "include" / "engine"
    secret.mkdir(parents=True)
    engine.mkdir()
    granted = [Path(path) for path in cage._interpreter_trees(home, [str(secret)], (engine,))]
    assert [path.name for path in granted] == ["bin"]
    assert cage._interpreter_trees(home, [str(prefix / "bin" / "inner")], ()) != cage._interpreter_trees(home, [], ())


def test_Interpreter_TheSettingsAndTheProfileGrantOnlyTheNarrowedFolders(repo, tmp_path, monkeypatch):
    prefix = tmp_path / "opt" / "py"
    _interpreter(monkeypatch, prefix)
    run_cage = _prepared(repo, home=HOME)
    real = str(Path(os.path.realpath(prefix)))
    allowed = json.loads(run_cage.settings.read_text())["sandbox"]["filesystem"]["allowRead"]
    assert real not in allowed and f"{real}/bin" in allowed
    assert f'(subpath "{real}")' not in run_cage.seatbelt.read_text()


# -- review 7: the object database's indirection points are not the agents' to write --------------------------------------

INDIRECTION_FILES = ("objects/info/alternates", "objects/info/http-alternates", "commondir", ".git")
PINNED_FOLDERS = ("objects", "objects/info", "objects/pack")


def test_Seatbelt_AnAgentCannotPointTheObjectDatabaseElsewhere_AndStillCommitsAndRepacks(repo, tmp_path):
    lane = tmp_path / "lane"
    git(repo, "worktree", "add", "-q", "-b", "lane", str(lane))
    common = repository.common_dir(repo)
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    run_cage = cage.prepare(_site(lane), cage.Policy("default"), 2, Path.home())
    for name in INDIRECTION_FILES:
        for command in (f"echo {elsewhere} > {common / name}", f"ln -s {elsewhere} {common / name}"):
            assert _caged(run_cage, command, lane).returncode != 0, command
            assert not os.path.lexists(common / name), command
    for name in PINNED_FOLDERS:
        assert _caged(run_cage, f"mv {common / name} {common / name}-moved", lane).returncode != 0, name
        assert (common / name).is_dir() and not (common / name).is_symlink(), name
    done = _caged(run_cage, "echo x > work.txt && git add work.txt && git -c core.hooksPath=/dev/null commit -qm caged "
                            "&& git -c gc.auto=0 repack -adq", lane)
    assert done.returncode == 0, done.stderr
    assert list((common / "objects" / "pack").glob("*.pack"))


def test_ClaudeSettings_DenyWritesToTheIndirectionFilesAndPinThePackFolder(repo):
    common = repository.common_dir(repo)
    deny = json.loads(_prepared(repo).settings.read_text())["sandbox"]["filesystem"]["denyWrite"]
    for name in INDIRECTION_FILES:
        assert str(common / name) in deny, name
    # Claude Code pins every folder above a write-denied path against removal and replacement: the alternates entries
    # pin `objects/info` and `objects`, a name Git never writes pins `objects/pack`.
    assert str(common / cage.PACK_FOLDER_PIN) in deny
    assert str(common / "objects") not in deny and str(common / "objects" / "pack") not in deny


def test_CodexProfile_GivesAReviewerNoWriteAnywhere(repo):
    table = _prepared(repo, role="code_reviewer").codex_args[3]
    assert '"write"' not in table and '"read"' in table


# -- review 7: toolchain grants never cover a credential file ---------------------------------------------------------

CARGO_GRANTS = (".cargo/bin", ".cargo/registry/index", ".cargo/registry/cache", ".cargo/registry/src", ".cargo/git/db",
                ".cargo/git/checkouts")
# A fake credential file at every known place, in or beside a granted tree; and plain files the toolchains read.
FAKE_CREDENTIALS = (".cargo/credentials", ".cargo/credentials.toml", ".cargo/config.toml", ".npmrc", ".pypirc",
                    ".netrc", ".git-credentials", ".docker/config.json", ".gem/credentials", ".gradle/gradle.properties",
                    ".m2/settings.xml", ".config/gh/hosts.yml", ".local/share/uv/credentials/credentials.toml",
                    ".nvm/versions/node/v20.0.0/etc/npmrc", ".pyenv/versions/3.12.0/pip.conf",
                    ".npm/.npmrc", ".rustup/.netrc", ".local/bin/.pypirc", ".cargo/registry/index/.git-credentials")
TOOLCHAIN_FILES = (".cargo/bin/tool", ".cargo/registry/index/entry", ".cargo/registry/src/crate.rs", ".rustup/settings.toml",
                   ".npm/_cacache/index", ".nvm/versions/node/v20.0.0/bin/node-tool", ".pyenv/versions/3.12.0/bin/tool")


def _toolchain_home(tmp_path):
    home = tmp_path / "home"
    for relative in (*FAKE_CREDENTIALS, *TOOLCHAIN_FILES):
        (home / relative).parent.mkdir(parents=True, exist_ok=True)
        (home / relative).write_text("credential-canary-11ab\n" if relative in FAKE_CREDENTIALS else "toolchain\n")
    return home


def test_Cage_TheCargoGrantIsItsMeasuredFolders_NeverTheWholeCargoHome(repo):
    run_cage = _prepared(repo)
    allowed = json.loads(run_cage.settings.read_text())["sandbox"]["filesystem"]["allowRead"]
    assert all(str(HOME / relative) in allowed for relative in CARGO_GRANTS)
    assert str(HOME / ".cargo") not in allowed
    assert f'(subpath "{HOME / ".cargo"}")' not in run_cage.seatbelt.read_text()
    assert f'"{HOME / ".cargo"}" = "read"' not in run_cage.codex_args[3]


def test_Seatbelt_EveryFakeCredentialFileInOrBesideAGrantedTree_IsUnreadable_TheToolchainFilesAreNot(repo, tmp_path):
    home = _toolchain_home(tmp_path)
    run_cage = cage.prepare(_site(repo), cage.Policy("default"), 2, home)
    for relative in FAKE_CREDENTIALS:
        done = _caged(run_cage, f"cat '{home / relative}'", repo)
        assert done.returncode != 0 and "credential-canary" not in done.stdout, relative
    for relative in TOOLCHAIN_FILES:
        done = _caged(run_cage, f"cat '{home / relative}'", repo)
        assert done.returncode == 0, (relative, done.stderr)


def test_Cage_EveryCredentialFile_IsASecretOfEveryEnforcementPoint(repo, tmp_path):
    home = _toolchain_home(tmp_path)
    run_cage = cage.prepare(_site(repo), cage.Policy("default"), 2, home)
    settings = json.loads(run_cage.settings.read_text())
    secrets = cage.secret_paths(cage.Policy("default"), home, repo)
    for relative in (".cargo/credentials", ".cargo/credentials.toml", ".npmrc", ".docker/config.json",
                     ".gradle/gradle.properties", ".m2/settings.xml", ".git-credentials", ".gem/credentials"):
        path = str(home / relative)
        assert path in secrets and path in settings["sandbox"]["filesystem"]["denyRead"], relative
        assert f"Read(/{path})" in settings["permissions"]["deny"], relative
        assert f'"{os.path.realpath(path)}" = "none"' in run_cage.codex_args[3], relative


def test_Cage_APolicyReadPathHoldingACredentialFile_IsRefused(repo):
    with pytest.raises(cage.CageError, match="stays denied"):
        _prepared(repo, policy=cage.Policy("maven", read_paths=("~/.m2",)))
    _prepared(repo, policy=cage.Policy("maven", read_paths=("~/.m2/repository",)))


def test_CodexProfile_AFakeCargoCredentialFile_IsUnreadable(repo):
    home = Path(os.path.realpath(Path.home()))
    for relative in (".cargo/credentials.toml", ".cargo/registry/index/.netrc"):
        (home / relative).parent.mkdir(parents=True, exist_ok=True)
        (home / relative).write_text("credential-canary-11ab\n")
    (home / ".cargo" / "bin").mkdir(parents=True, exist_ok=True)
    (home / ".cargo" / "bin" / "tool").write_text("toolchain\n")
    done = _codex_sandbox(repo, f"cat {home / '.cargo/credentials.toml'}; cat {home / '.cargo/registry/index/.netrc'}; "
                                f"cat {home / '.cargo/bin/tool'}")
    assert "credential-canary" not in done.stdout and "toolchain" in done.stdout, done.stdout + done.stderr


# -- review 10: no role writes a workspace's `.git` marker or a project record -----------------------------------------

def _lane_cage(repo, tmp_path):
    lane = tmp_path / "lane"
    git(repo, "worktree", "add", "-q", "-b", "lane", str(lane))
    return lane, cage.prepare(_site(lane), cage.Policy("default"), 2, Path.home())


def test_Seatbelt_AWorkspacesGitMarker_CannotBeRewrittenRemovedRenamedOrLinkedAway(repo, tmp_path):
    lane, run_cage = _lane_cage(repo, tmp_path)
    marker = lane / ".git"
    before = marker.read_bytes()
    fake = tmp_path / "fake.git"
    fake.mkdir()
    for command in (f"echo 'gitdir: {fake}' > .git", "rm -f .git", "mv .git moved", f"ln -sf {fake} .git",
                    "ln .git linked && echo 'gitdir: /elsewhere' > linked"):
        assert _caged(run_cage, command, lane).returncode != 0, command
        assert marker.is_file() and not marker.is_symlink() and marker.read_bytes() == before, command


def test_Seatbelt_ANestedGitMarker_CannotBeMadeAnywhereInTheWorkspaceOrTheTasksGitFolder(repo, tmp_path):
    lane, run_cage = _lane_cage(repo, tmp_path)
    common = repository.common_dir(repo)
    for folder in (lane / "sub", common / "refs" / "fake"):
        commands = (f"mkdir -p {folder} && echo 'gitdir: /elsewhere' > {folder / '.git'}",
                    f"mkdir -p {folder / '.git'}",
                    f"mkdir -p {folder / 'made'} && mv {folder / 'made'} {folder / '.git'}")
        for command in commands:
            assert _caged(run_cage, command, lane).returncode != 0, command
            assert not os.path.lexists(folder / ".git"), command


def test_Seatbelt_AProjectRecord_CannotBeWrittenAnywhereARoleWrites(repo, tmp_path):
    lane, run_cage = _lane_cage(repo, tmp_path)
    common = repository.common_dir(repo)
    record = engine_home.TASK_REPOSITORY_RECORD
    for folder in (common, common / "refs" / "fake", common / "worktrees" / "lane", lane, lane / "nested" / "x.git",
                   run_cage.tmp / "fake.git"):
        assert _caged(run_cage, f"mkdir -p {folder} && echo {repo / '.git'} > {folder / record}", lane).returncode != 0
        assert not os.path.lexists(folder / record), folder


def test_Seatbelt_WithTheMarkersDenied_ALaneStillCommitsChecksOutAndStashes(repo, tmp_path):
    lane, run_cage = _lane_cage(repo, tmp_path)
    work = ("echo x > work.txt && git add work.txt && git -c core.hooksPath=/dev/null commit -qm caged && "
            "git checkout -qb other && echo y > work.txt && git stash -q && git stash pop -q && git status --short")
    done = _caged(run_cage, work, lane)
    assert done.returncode == 0, done.stderr
    assert "work.txt" in done.stdout


def test_ClaudeSettings_DenyWritesToEveryGitMarkerAndProjectRecord(repo, tmp_path):
    lane, run_cage = _lane_cage(repo, tmp_path)
    deny = json.loads(run_cage.settings.read_text())["sandbox"]["filesystem"]["denyWrite"]
    common, record = repository.common_dir(repo), engine_home.TASK_REPOSITORY_RECORD
    for path in (lane / ".git", lane / "**" / ".git", common / "**" / ".git", lane / "**" / record,
                 common / "**" / record, run_cage.tmp / "**" / record):
        assert str(path) in deny, path


def test_Seatbelt_ACaseVariantOfAMarkerOrRecord_IsRefusedToo(repo, tmp_path):
    # Measured: on a case-insensitive volume Seatbelt folds case for literal, subpath and regex rules alike.
    lane, run_cage = _lane_cage(repo, tmp_path)
    common = repository.common_dir(repo)
    before = (lane / ".git").read_bytes()
    for command in ("echo 'gitdir: /elsewhere' > .GIT", "mkdir -p sub && echo x > sub/.Git", "mkdir -p sub/.gIT",
                    f"mkdir -p {common / 'refs' / 'x'} && echo x > {common / 'refs' / 'x' / 'SAKURA-PROJECT'}",
                    f"echo x > {run_cage.tmp / 'Sakura-Project'}"):
        assert _caged(run_cage, command, lane).returncode != 0, command
    assert (lane / ".git").read_bytes() == before
    assert not [path for path in (*lane.rglob("*"), *common.rglob("*"), *run_cage.tmp.rglob("*"))
                if path.name.lower() in (".git", engine_home.TASK_REPOSITORY_RECORD) and path != lane / ".git"]


def test_Seatbelt_AMarkerCannotBeBuiltInTheTmpFolderAndMovedIn(repo, tmp_path):
    lane, run_cage = _lane_cage(repo, tmp_path)
    tmp = run_cage.tmp
    for command in (f"mkdir -p {tmp / 'x'} && echo 'gitdir: /elsewhere' > {tmp / 'x' / '.git'}",
                    f"mkdir -p {tmp / 'y' / '.git'}",
                    f"mkdir -p {tmp / 'z' / 'made'} && mv {tmp / 'z' / 'made'} {tmp / 'z' / '.git'}"):
        assert _caged(run_cage, command, lane).returncode != 0, command
    _caged(run_cage, f"mv {tmp / 'x'} {lane / 'x'}; mv {tmp / 'z'} {lane / 'z'}", lane)
    assert not [path for path in (*lane.rglob(".git"), *tmp.rglob(".git")) if path != lane / ".git"]


def test_ClaudeSettings_DenyMarkersInTheTmpFolderToo(repo, tmp_path):
    lane, run_cage = _lane_cage(repo, tmp_path)
    deny = json.loads(run_cage.settings.read_text())["sandbox"]["filesystem"]["denyWrite"]
    assert str(run_cage.tmp / "**" / ".git") in deny


def test_ClaudeSettings_DenyMarkersAndRecordsInTheSystemsTmpFoldersToo(repo, tmp_path):
    # Measured in a Claude lane (2.1.323): its commands get TMPDIR=/private/tmp/claude-<uid>, a folder Claude Code's own
    # sandbox lets them write, so a marker built there could be moved into the workspace.
    import tempfile

    lane, run_cage = _lane_cage(repo, tmp_path)
    deny = json.loads(run_cage.settings.read_text())["sandbox"]["filesystem"]["denyWrite"]
    for root in ("/private/tmp", "/private/var/tmp", os.path.realpath(tempfile.gettempdir())):
        for name in (".git", engine_home.TASK_REPOSITORY_RECORD):
            assert f"{root}/**/{name}" in deny, (root, name)


# -- review 11, finding 2: a run isolated from the task (the reporter) ------------------------------------------------

def test_IsolatedRun_DeniesTheTasksFoldersToEveryToolAndGetsATmpFolderOfItsOwn(repo, tmp_path):
    zero, task_repository_folder = tmp_path / "trees" / "demo-0", tmp_path / "trees" / "demo.git"
    shared = cage.task_tmp_dir(engine_home.root() / "proj" / "flow" / "demo")
    own = cage.new_isolated_tmp_dir(engine_home.root() / "proj" / "flow" / "demo")
    site = cage.RunSite(engine_home.root() / "proj" / "flow" / "demo", "report-1", "reporter", repo,
                        repository.common_dir(repo), (engine_home.root(), engine_home.account_root()), repo,
                        tmp_dir=own, denied_reads=(zero, task_repository_folder, shared))
    run_cage = cage.prepare(site, cage.Policy("default"), 0, HOME)
    settings = json.loads(run_cage.settings.read_text())
    deny_read, deny_write = settings["sandbox"]["filesystem"]["denyRead"], settings["sandbox"]["filesystem"]["denyWrite"]
    rules = settings["permissions"]["deny"]
    for folder in (zero, task_repository_folder, shared):
        assert str(folder) in deny_read and str(folder) in deny_write
        assert f"Read(/{folder}/**)" in rules and f"Edit(/{folder}/**)" in rules
        assert f'(subpath "{folder}")' in run_cage.seatbelt.read_text()
    assert run_cage.tmp == own != shared and own.parent == shared.parent and own.is_dir()
    assert str(shared) not in settings["sandbox"]["filesystem"]["allowWrite"]
    other = cage.prepare(_site(repo, "builder"), cage.Policy("default"), 0, HOME)
    assert other.tmp == shared  # every other run keeps the task's shared folder
    shutil.rmtree(own)  # the runner removes an isolated run's folder when the run ends


# -- review 15: role-writable roots the engine reuses ---------------------------------------------------------------


def _replace_with_link(folder: Path, target: Path) -> None:
    """What caged code can do between two steps: move the folder it may write away and put a link at its name."""
    folder.rename(folder.with_name(folder.name + "-moved"))
    folder.symlink_to(target, target_is_directory=True)


def _cage_files(run_cage) -> dict:
    return {path.name: path.read_text() for path in (run_cage.settings, run_cage.seatbelt, run_cage.policy)}


def test_Cage_ASharedTmpFolderReplacedByALinkToTheEngineHome_IsRefusedAndGrantsNothing(repo, tmp_path):
    # Review 15, finding 1: a cage never resolves a role-writable folder through a link into a grant.
    first = _prepared(repo)
    written = _cage_files(first)
    target = engine_home.root()
    target.mkdir(parents=True, exist_ok=True)
    _replace_with_link(first.tmp, target)

    with pytest.raises(cage.CageError, match="replaced"):
        _prepared(repo)
    assert _cage_files(first) == written, "a refused cage writes no grant"
    assert not (target / "config").exists() and not (target / "claude").exists()


def test_Cage_ABuildFolderReplacedByALinkBetweenSteps_IsRefused(repo, tmp_path):
    build = cage.new_build_dir()
    site = dataclasses.replace(_site(repo, "checks", "checks-1"), build_dir=build)
    cage.prepare(site, cage.Policy("default"), 0, HOME)
    target = engine_home.root()
    target.mkdir(parents=True, exist_ok=True)
    _replace_with_link(build, target)
    try:
        with pytest.raises(cage.CageError, match="replaced"):
            cage.prepare(site, cage.Policy("default"), 0, HOME)
    finally:
        build.unlink()
        shutil.rmtree(build.with_name(build.name + "-moved"), ignore_errors=True)


def test_Cage_AnIsolatedTmpFolderReplacedByALink_IsRefused(repo, tmp_path):
    own = cage.new_isolated_tmp_dir(_site(repo).task_root)
    site = dataclasses.replace(_site(repo, "code_reviewer", "review-1"), tmp_dir=own)
    cage.prepare(site, cage.Policy("default"), 0, HOME)
    _replace_with_link(own, tmp_path)
    with pytest.raises(cage.CageError, match="replaced"):
        cage.prepare(site, cage.Policy("default"), 0, HOME)


def test_Cage_TheSeatbeltProfileDeniesReplacingTheWritableRootsThemselves(repo):
    run_cage = _prepared(repo)
    profile = run_cage.seatbelt.read_text()
    tmp = os.path.realpath(run_cage.tmp)
    assert any(line.startswith("(deny file-write* (literal") and f'(literal "{tmp}")' in line
               for line in profile.splitlines())


def test_Cage_TheGitCommonFolderReplacedByALink_IsRefused(repo, tmp_path):
    # Security review of the pins: the Git common folder is writable and reused like the workspace and tmp folder.
    site = _site(repo)
    cage.prepare(site, cage.Policy("default"), 2, HOME)
    target = tmp_path / "elsewhere"
    target.mkdir()
    _replace_with_link(site.common_dir, target)
    with pytest.raises(cage.CageError, match="replaced"):
        cage.prepare(site, cage.Policy("default"), 2, HOME)


def test_Cage_TheRootDenialsNameEveryRootByTheSpellingSeatbeltMatches(repo):
    # Seatbelt matches resolved paths (`/var` is `/private/var`): a root spelled through a link is denied as resolved.
    real = Path(os.path.realpath(repo))
    if not str(real).startswith("/private/var/"):
        pytest.skip("the test repository is not under /private/var")
    alias = Path("/var") / real.relative_to("/private/var")
    site = dataclasses.replace(_site(repo), workspace=alias)
    run_cage = cage.prepare(site, cage.Policy("default"), 2, HOME)
    profile = run_cage.seatbelt.read_text()
    common = os.path.realpath(repository.common_dir(repo))
    roots_line = next(line for line in profile.splitlines()
                      if line.startswith("(deny file-write* (literal") and f'"{real}"' in line)
    for root in (str(real), common, os.path.realpath(run_cage.tmp)):
        assert f'(literal "{root}")' in roots_line, root


# -- a builder run under the test interpreter can start it (the brake of a commit runs it inside the cage)

def test_Cage_ACommandUnderTheTestInterpreterCanStartIt(repo, tmp_path, monkeypatch):
    from sakura import interpreter

    fixture = Path(__file__).resolve().parent / "fixtures" / "trust_root_python"
    monkeypatch.setenv(interpreter.ENV_NAME, str(fixture))
    monkeypatch.setenv("SAKURA_TEST_TRUST_ROOT", str(tmp_path / "trust"))
    monkeypatch.setenv("SAKURA_TEST_LIB", str(Path(__file__).resolve().parents[1] / "lib"))
    run_cage = _prepared(repo)

    done = _caged(run_cage, '"$SAKURA_PYTHON" -c "print(1 + 1)"', repo, extra_env=interpreter.environment())

    assert done.returncode == 0 and done.stdout.strip() == "2", done.stderr


def test_Cage_TheTestInterpreterIsReadableOnlyWhenTheEngineHandsItOn(repo, monkeypatch):
    from sakura import interpreter

    fixture = Path(__file__).resolve().parent / "fixtures" / "trust_root_python"
    run_cage = _prepared(repo)
    site = _site(repo)
    monkeypatch.delenv("SAKURA_TEST_TRUST_ROOT", raising=False)
    assert str(fixture) not in cage.readable_paths(site, cage.Policy("default"), run_cage, HOME)

    monkeypatch.setenv(interpreter.ENV_NAME, str(fixture))
    monkeypatch.setenv("SAKURA_TEST_TRUST_ROOT", "/somewhere")
    assert str(fixture) in cage.readable_paths(site, cage.Policy("default"), run_cage, HOME)


def test_Cage_ABuilderUnderTheTestInterpreterCommitsThroughTheEnginesCommitHook(repo, tmp_path, monkeypatch):
    from sakura import install, interpreter

    fixture = Path(__file__).resolve().parent / "fixtures" / "trust_root_python"
    monkeypatch.setenv(interpreter.ENV_NAME, str(fixture))
    monkeypatch.setenv("SAKURA_TEST_TRUST_ROOT", str(tmp_path / "trust"))
    monkeypatch.setenv("SAKURA_TEST_LIB", str(Path(__file__).resolve().parents[1] / "lib"))
    install.install(repo)  # the engine's hooks, as in every workspace
    git(repo, "checkout", "-q", "-b", "lane")  # the brake refuses commits on the main branch
    run_cage = _prepared(repo)
    flow = {"SAKURA_FLOW_ROLE": "builder", "SAKURA_FLOW_TASK": "demo", "SAKURA_FLOW_RUN": "build-1",
            "SAKURA_FLOW_PHASE": "build"}

    done = _caged(run_cage, "echo x > w.txt && git add w.txt && git commit -qm caged", repo,
                  extra_env={**interpreter.environment(), **flow})

    assert done.returncode == 0, done.stderr
    assert "Operation not permitted" not in done.stderr


# -- Codex roles that write: the same limits as the Claude cage, in Codex's own profile -----------------------------

ACCESS = re.compile(r'"([^"]+)" = "(\w+)"')


def _codex_table(run_cage, profile="sakura-work"):
    text = next(arg for arg in run_cage.codex_args if arg.startswith(f"permissions.{profile}.filesystem="))
    return {path: access for path, access in ACCESS.findall(text)}


def _codex_setting(run_cage, key):
    return next(arg.partition("=")[2] for arg in run_cage.codex_args if arg.startswith(key + "="))


def _writes(table):
    return {path for path, access in table.items() if access == "write"}


def test_CodexProfile_ABuilder_WritesItsWorkspaceItsGitFolderAndItsTmpAndNothingElse(repo):
    run_cage = _prepared(repo, role="builder")
    assert 'default_permissions="sakura-work"' in run_cage.codex_args
    real = os.path.realpath
    assert _writes(_codex_table(run_cage)) == {real(repo), real(repository.common_dir(repo)), real(run_cage.tmp)}


def test_CodexProfile_ALinkedWorktree_AlsoWritesItsOwnFolderInsideTheGitFolder(repo, tmp_path):
    worktree = tmp_path / "linked"
    git(repo, "worktree", "add", "-q", "-b", "side", str(worktree))
    site = dataclasses.replace(_site(repo), workspace=worktree, common_dir=repository.common_dir(repo))
    run_cage = cage.prepare(site, cage.Policy("default"), 2, HOME)
    real = os.path.realpath
    assert real(repository.common_dir(repo) / "worktrees" / "linked") in _writes(_codex_table(run_cage))
    assert real(repo) not in _writes(_codex_table(run_cage))


@pytest.mark.parametrize("role", ["planner", "builder", "evaluator", "reporter"])
def test_CodexProfile_EveryRoleThatWrites_GetsTheWorkProfile(repo, role):
    assert 'default_permissions="sakura-work"' in _prepared(repo, role=role).codex_args


@pytest.mark.parametrize("role", ["plan_reviewer", "code_reviewer"])
def test_CodexProfile_AReviewer_KeepsTheReadOnlyProfile(repo, role):
    run_cage = _prepared(repo, role=role)
    assert 'default_permissions="sakura-review"' in run_cage.codex_args
    assert "write" not in _codex_table(run_cage, "sakura-review").values()


def test_CodexProfile_TheGitHooksAndConfigInsideTheWritableGitFolder_StayReadOnly(repo):
    table = _codex_table(_prepared(repo, role="builder"))
    common = os.path.realpath(repository.common_dir(repo))
    for name in ("hooks", "config", "config.worktree"):
        assert table[f"{common}/{name}"] == "read", name


def test_CodexProfile_SecretsAndEngineFoldersStayDeniedOverEveryWriteRoot(repo):
    run_cage = _prepared(repo, role="builder")
    table = _codex_table(run_cage)
    for relative in (".ssh", ".aws", ".claude", ".codex"):
        assert table[os.path.realpath(HOME / relative)] == "none"
    assert table[os.path.realpath(engine_home.root())] == "none"


def test_CodexProfile_ARowRun_WritesOnlyItsOwnBuildFolderAndTmp(repo, tmp_path):
    build = cage.new_build_dir()
    try:
        site = dataclasses.replace(_site(repo, "evaluator"), build_dir=build)
        run_cage = cage.prepare(site, cage.Policy("default"), 2, HOME)
        real = os.path.realpath
        assert _writes(_codex_table(run_cage)) == {real(run_cage.tmp), real(build)}
    finally:
        shutil.rmtree(build, ignore_errors=True)


def test_CodexProfile_ABuilderGetsTheNetworkPolicyOfTheClaudeCage_ThroughCodexsOwnProxy(repo):
    run_cage = _prepared(repo, role="builder")
    assert "features.network_proxy=true" in run_cage.codex_args
    network = _codex_setting(run_cage, "permissions.sakura-work.network")
    assert "enabled=true" in network and '"api.github.com" = "allow"' in network.replace("=", " = ").replace("  ", " ")
    assert "allow_local_binding=false" in network


def test_CodexProfile_AReviewerHasNoNetwork(repo):
    run_cage = _prepared(repo, role="code_reviewer")
    assert "permissions.sakura-review.network={enabled=false}" in run_cage.codex_args
    assert not any(arg.startswith("features.network_proxy") for arg in run_cage.codex_args)


def test_CodexProfile_LocalPortsAreOpenOnlyWhenThePersonApprovedLocalSockets_NeverForJustTheRowsExactPorts(repo):
    for site, expected in ((dataclasses.replace(_site(repo, "evaluator"), loopback_ports=(8123,)), "false"),
                           (dataclasses.replace(_site(repo, "builder"), local_sockets=True), "true")):
        network = _codex_setting(cage.prepare(site, cage.Policy("default"), 2, HOME), "permissions.sakura-work.network")
        assert f"allow_local_binding={expected}" in network


def test_CodexMcp_ARoleOfAGrantedClass_GetsTheEnginesToolServerThroughCodexConfig(repo):
    run_cage = _prepared(repo, role="builder")
    server = _codex_setting(run_cage, "mcp_servers.sakura")
    policy = json.loads(run_cage.mcp_config.read_text())["mcpServers"]["sakura"]
    assert policy["command"] in server and str(run_cage.policy) in server and "cage" in server


def test_CodexProfile_TheWorkspacesGitMarkerStaysReadable_NoGlobDeniesItSelf(repo, tmp_path):
    worktree = tmp_path / "linked"
    git(repo, "worktree", "add", "-q", "-b", "side2", str(worktree))
    site = dataclasses.replace(_site(repo), workspace=worktree, common_dir=repository.common_dir(repo))
    table = _codex_table(cage.prepare(site, cage.Policy("default"), 2, HOME))
    marker = os.path.realpath(worktree) + "/.git"
    assert table[marker] == "read"
    assert not [path for path, access in table.items() if path.endswith("/**/.git") and access == "none"]
    assert any(path.endswith("/**/" + engine_home.TASK_REPOSITORY_RECORD) for path in table)


def _children(folder, keep):
    return sorted(entry for entry in os.listdir(folder) if entry not in keep)


@pytest.mark.parametrize("folder, keep", [("/Applications", ("Xcode.app",)),
                                          ("/Library/Preferences", ("com.apple.dt.Xcode.plist",))])
def test_CodexProfile_AWorkRoleReadsOnlyWhatTheDeveloperToolsNeedOfAFolderTheReviewerCannotRead(repo, folder, keep):
    # A glob deny beats a narrower read (measured), so the folder cannot be denied whole and read back: every other entry is.
    work_args, review_args = " ".join(_prepared(repo, role="builder").codex_args), " ".join(_prepared(repo, role="code_reviewer").codex_args)
    assert f'"{folder}/**" = "none"' in review_args and f'"{folder}/**" = "none"' not in work_args
    others = _children(folder, keep)
    assert others, "this host has nothing else in the folder to check"
    for entry in others:
        assert f'"{folder}/{entry}/**" = "none"' in work_args, entry
    for entry in keep:
        assert f'"{folder}/{entry}' not in " ".join(part for part in work_args.split(", ") if part.endswith('= "none"'))



def test_CodexProfile_AWorkRoleMayReadTheApplicationsFolderForXcode_AReviewerMayNot(repo):
    work = _prepared(repo, role="builder")
    review = _prepared(repo, role="code_reviewer")
    work_args, review_args = " ".join(work.codex_args), " ".join(review.codex_args)
    assert '"/Applications*" = "none"' not in work_args and '"/Applications/**" = "none"' not in work_args
    assert '"/Applications*" = "none"' in review_args and '"/Applications/**" = "none"' in review_args
    assert '"/Applications/Xcode.app" = "read"' in work_args


def test_CodexShell_PathIsTheEnginesBinFirstThenOnlyFoldersTheProfileCanRead(repo):
    run_cage = _prepared(repo, role="builder")
    setting = next(arg for arg in run_cage.codex_args if arg.startswith("shell_environment_policy.set="))
    path = re.search(r'PATH="([^"]*)"', setting).group(1).split(":")
    assert path[0] == str(KIT_BIN.parent) and "/usr/bin" in path and "/bin" in path
    table = _codex_table(run_cage)
    readable = {entry for entry, access in table.items() if access in ("read", "write")}
    assert all(os.path.realpath(folder) in readable or folder in ("/usr/bin", "/bin", "/usr/sbin", "/sbin", "/usr/local/bin")
               for folder in path)


def test_evaluator_row_context_alone_offers_no_run_row_tool(repo, tmp_path):
    policy = _policy(repo, tmp_path, role="evaluator", tools=[], run_row={"cwd": str(repo), "env": ROW_ENV})
    listing = {"jsonrpc": "2.0", "id": 1, "method": "tools/list"}

    [reply] = _session(policy, listing)

    assert {tool["name"] for tool in reply["result"]["tools"]} == {"permission"}


def test_permission_audit_keeps_raw_tool_input_private(repo, tmp_path):
    policy = _policy(repo, tmp_path)
    cage_server.permission_answer(policy, {"tool_name": "Bash", "input": {"command": "audit-fixture-document"}})

    assert "audit-fixture-document" not in Path(policy["requests"]).read_text()


def test_cage_uses_the_shared_bounded_stdio_dispatcher(repo, tmp_path, monkeypatch):
    from sakura import mcp_stdio

    seen = []
    monkeypatch.setattr(mcp_stdio, "serve", lambda name, tools, stdin, stdout: seen.append(name))
    cage_server.serve(_policy(repo, tmp_path), io.StringIO(), io.StringIO())

    assert seen == ["sakura"]


def test_cage_snapshots_only_selected_connector_authority(repo):
    from datetime import UTC, datetime

    from sakura import grants
    from sakura.connectors import kinds, registry

    selected = registry.mutate("add", "add", kinds.Ports(), datetime.now(UTC),
                               fields={"kind": "feed", "label": "News", "address": "https://example.test"})["id"]
    site = dataclasses.replace(_site(repo, role="master"), connector_ids=(selected,))
    granted = grants.ClassGrant(connectors={selected: ("read", "comment_pr"), "cn-other": ("read",)})
    policy = cage.server_policy(site, _prepared(repo), granted)
    assert policy["connectors"] == {selected: {"revision": 1, "operations": ["read"]}}
    assert "https://" not in json.dumps(policy["connectors"])
