from __future__ import annotations

import subprocess
import sys
from types import SimpleNamespace

import pytest
from tmux_support import TMUX_PROGRAM, needs_tmux, private_tmux, wait_until

from sakura.flow import tmux


MARKER = ("@sakura-task", "/repo:demo")
BRACKET_START = b"\x1b[200~"
BRACKET_END = b"\x1b[201~"
PSEUDO_AGENT = """
import os, sys, tty
tty.setraw(0)
sys.stdout.write("\\x1b[?2004hAGENT-READY")
sys.stdout.flush()
with open(sys.argv[1], "wb", buffering=0) as record:
    while True:
        chunk = os.read(0, 4096)
        if not chunk:
            break
        record.write(chunk)
"""


class Recorder:
    """Stands in for subprocess.run: records argv lists and answers has-session as missing."""

    def __init__(self):
        self.calls = []
        self.kwargs = []

    def __call__(self, argv, **kwargs):
        self.calls.append(list(argv))
        self.kwargs.append(kwargs)
        missing = "has-session" in argv
        return SimpleNamespace(returncode=1 if missing else 0, stdout="", stderr="")


def test_argv_lists_with_socket_and_dash_e(tmp_path):
    recorder = Recorder()
    client = tmux.TmuxClient(socket="sock", run=recorder)
    created = client.ensure_session("demo-abc123", tmp_path, "events", ["tail", "-f", "x;"],
                                    {"ALPHA": "1", "BETA": "b;"}, MARKER)
    assert created is True
    new_session = next(call for call in recorder.calls if "new-session" in call)
    assert new_session[:4] == ["tmux", "-L", "sock", "new-session"]
    assert new_session.count("-e") == 2
    assert new_session[new_session.index("ALPHA=1") - 1] == "-e"
    assert "BETA=b\\;" in new_session and "x\\;" in new_session
    assert new_session[-4:] == ["--", "tail", "-f", "x\\;"]
    assert all(isinstance(call, list) and kwargs.get("shell") is not True
               for call, kwargs in zip(recorder.calls, recorder.kwargs))


def test_no_socket_means_default_server(tmp_path):
    recorder = Recorder()
    tmux.TmuxClient(run=recorder).has_session("demo-abc123")
    assert recorder.calls[0][:2] == ["tmux", "has-session"]


@pytest.mark.parametrize("code, running", [(0, True), (1, False)])
def test_has_server_asks_with_a_command_that_never_starts_one(code, running):
    calls = []

    def run(argv, **kwargs):
        calls.append(list(argv))
        return SimpleNamespace(returncode=code, stdout="", stderr="")

    assert tmux.TmuxClient(socket="sock", run=run).has_server() is running
    assert calls == [["tmux", "-L", "sock", "list-sessions"]]


def test_hostile_names_and_env_keys_are_refused(tmp_path):
    client = tmux.TmuxClient(run=Recorder())
    with pytest.raises(tmux.TmuxError):
        client.ensure_session("demo;kill-server", tmp_path, "events", ["true"], {}, MARKER)
    with pytest.raises(tmux.TmuxError):
        client.ensure_session("demo", tmp_path, "events", ["true"], {"A B": "1"}, MARKER)
    with pytest.raises(tmux.TmuxError):
        client.set_option("demo", "status", "off")


def test_sanitize_name_keeps_the_safe_charset_and_caps_length():
    assert tmux.sanitize_name("ab C/d;e") == "ab-C-d-e"
    assert tmux.sanitize_name("x" * 100) == "x" * 60
    assert tmux.sanitize_name("ok._-9") == "ok._-9"


def test_marked_names_differ_when_sanitized_bases_collide():
    first = tmux.marked_name("a b", "/repo-one:task")
    second = tmux.marked_name("a/b", "/repo-two:task")
    assert first.startswith("a-b-") and second.startswith("a-b-")
    assert first != second
    assert first == tmux.marked_name("a b", "/repo-one:task")
    assert len(first.rsplit("-", 1)[1]) == 6


def test_switch_or_attach_follows_whether_we_are_inside_this_clients_server(monkeypatch):
    client = tmux.TmuxClient(socket="sock", run=Recorder())
    monkeypatch.delenv("TMUX", raising=False)
    assert client.switch_or_attach("demo") == ["tmux", "-L", "sock", "attach-session", "-t", "=demo:"]
    monkeypatch.setenv("TMUX", "/tmp/tmux-1/sock,1,0")
    assert client.switch_or_attach("demo") == ["tmux", "-L", "sock", "switch-client", "-t", "=demo:"]
    # Inside the person's own tmux, another server: attach from outside it, never switch a client we do not have.
    monkeypatch.setenv("TMUX", "/tmp/tmux-1/default,1,0")
    assert client.switch_or_attach("demo") == ["env", "-u", "TMUX", "tmux", "-L", "sock", "attach-session", "-t", "=demo:"]


def test_the_default_surface_client_uses_the_private_sakura_socket_never_the_default_server():
    assert tmux.surface_client().socket == "sakura"
    assert tmux.surface_client()._base() == ["tmux", "-L", "sakura", "-f", "/dev/null"]


def test_the_private_server_reads_no_config_file_so_a_session_restoring_plugin_cannot_run_in_it():
    recorder = Recorder()
    client = tmux.TmuxClient(socket="sakura", run=recorder, config_file="/dev/null")
    client.has_server()
    assert recorder.calls[0][:5] == ["tmux", "-L", "sakura", "-f", "/dev/null"]


@needs_tmux
def test_ensure_session_first_window_no_stray_shell(tmp_path):
    with private_tmux() as client:
        assert client.ensure_session("demo-abc123", tmp_path, "events", ["sleep", "30"], {}, MARKER) is True
        assert [window.name for window in client.windows("demo-abc123")] == ["events"]
        assert client.option("demo-abc123", MARKER[0]) == MARKER[1]


@needs_tmux
def test_ensure_session_is_idempotent_for_the_same_marker(tmp_path):
    with private_tmux() as client:
        client.ensure_session("demo-abc123", tmp_path, "events", ["sleep", "30"], {}, MARKER)
        assert client.ensure_session("demo-abc123", tmp_path, "other", ["sleep", "30"], {}, MARKER) is True
        assert [window.name for window in client.windows("demo-abc123")] == ["events"]


@needs_tmux
def test_ensure_session_refuses_foreign_marker(tmp_path):
    with private_tmux() as client:
        client.ensure_session("demo-abc123", tmp_path, "events", ["sleep", "30"], {}, MARKER)
        assert client.ensure_session("demo-abc123", tmp_path, "other", ["sleep", "30"], {},
                                     ("@sakura-task", "/elsewhere:demo")) is False
        assert client.option("demo-abc123", MARKER[0]) == MARKER[1]
        assert [window.name for window in client.windows("demo-abc123")] == ["events"]


@needs_tmux
def test_session_names_with_dots_are_found_again(tmp_path):
    with private_tmux() as client:
        client.ensure_session("my.repo-abc123", tmp_path, "events", ["sleep", "30"], {}, MARKER)
        assert client.has_session("my.repo-abc123")
        assert client.option("my.repo-abc123", MARKER[0]) == MARKER[1]


@needs_tmux
def test_kill_session_checks_marker(tmp_path):
    with private_tmux() as client:
        client.ensure_session("demo-abc123", tmp_path, "events", ["sleep", "30"], {}, MARKER)
        assert client.kill_session("demo-abc123", ("@sakura-task", "/elsewhere:demo")) is False
        assert client.has_session("demo-abc123")
        assert client.kill_session("demo-abc123", MARKER) is True
        assert not client.has_session("demo-abc123")
        assert client.kill_session("demo-abc123", MARKER) is False


@needs_tmux
def test_unset_option_reads_empty_and_user_options_round_trip(tmp_path):
    with private_tmux() as client:
        client.ensure_session("demo-abc123", tmp_path, "events", ["sleep", "30"], {}, MARKER)
        assert client.option("demo-abc123", "@sakura-missing") == ""
        client.set_option("demo-abc123", "@sakura-note", "-rf; echo 'x';")
        assert client.option("demo-abc123", "@sakura-note") == "-rf; echo 'x';"


@needs_tmux
def test_new_window_and_split_window_carry_markers_cwd_and_env(tmp_path):
    odd = tmp_path / "dir#{pane_id}x"
    odd.mkdir()
    out = tmp_path / "out.txt"
    script = f"import os; open({str(out)!r}, 'w').write(os.getcwd() + '|' + os.environ['FLOW_X'])"
    with private_tmux() as client:
        client.ensure_session("demo-abc123", tmp_path, "events", ["sleep", "30"], {}, MARKER)
        window = client.new_window("demo-abc123", "build-1", [sys.executable, "-c", script], odd,
                                   {"FLOW_X": "v;#{pane_id}"})
        pane = client.split_window(window, ["sleep", "30"], tmp_path, {}, "stack")
        assert wait_until(out.exists)
        assert out.read_text() == f"{odd.resolve()}|v;#{{pane_id}}"
        found = next(item for item in client.windows("demo-abc123") if item.id == window)
        assert found.name == "build-1" and len(found.panes) == 2
        assert {entry.id: entry.marker for entry in found.panes}[pane] == "stack"


@needs_tmux
def test_remain_on_exit_window_shows_dead(tmp_path):
    with private_tmux() as client:
        client.ensure_session("demo-abc123", tmp_path, "events", ["sleep", "30"], {}, MARKER)
        kept = client.new_window("demo-abc123", "done-1", ["true"], tmp_path, {})
        gone = client.new_window("demo-abc123", "done-2", ["true"], tmp_path, {}, remain=False)

        def kept_is_dead():
            return {w.id: w for w in client.windows("demo-abc123")}.get(kept, SimpleNamespace(dead=False)).dead

        assert wait_until(kept_is_dead)
        assert wait_until(lambda: gone not in {w.id for w in client.windows("demo-abc123")})
        live = next(w for w in client.windows("demo-abc123") if w.name == "events")
        assert live.dead is False
        client.kill_window(kept)
        assert kept not in {w.id for w in client.windows("demo-abc123")}


@needs_tmux
def test_window_options_are_listed(tmp_path):
    with private_tmux() as client:
        client.ensure_session("demo-abc123", tmp_path, "events", ["sleep", "30"], {}, MARKER)
        window = client.new_window("demo-abc123", "lane-a", ["sleep", "30"], tmp_path, {})
        client.set_option(window, "@sakura-lane", "alpha")
        found = next(item for item in client.windows("demo-abc123") if item.id == window)
        assert found.options == {"@sakura-lane": "alpha"}


@needs_tmux
def test_pane_title_reads_the_title_the_program_set(tmp_path):
    script = "import sys,time; sys.stdout.write('\\x1b]2;my title\\x07'); sys.stdout.flush(); time.sleep(30)"
    with private_tmux() as client:
        client.ensure_session("demo-abc123", tmp_path, "events", [sys.executable, "-c", script], {}, MARKER)
        pane = client.windows("demo-abc123")[0].panes[0].id
        assert wait_until(lambda: client.pane_title(pane) == "my title")


@needs_tmux
def test_paste_delivers_exact_bytes(tmp_path):
    record = tmp_path / "record.bin"
    text = "-rf; echo hi;\nsecond 'line' \"q\" $HOME `x` #{pane_id}\r\nlast;"
    with private_tmux() as client:
        client.ensure_session("demo-abc123", tmp_path, "agent", [sys.executable, "-c", PSEUDO_AGENT, str(record)],
                              {}, MARKER)
        pane = client.windows("demo-abc123")[0].panes[0].id
        assert wait_until(lambda: "AGENT-READY" in _capture(client, pane))
        client.paste(pane, text)
        client.send_enter(pane)
        expected = BRACKET_START + text.encode() + BRACKET_END + b"\r"
        assert wait_until(lambda: record.exists() and record.read_bytes() == expected), record.read_bytes()
        assert _buffers(client) == ""


def test_paste_refuses_empty_and_nul_text():
    client = tmux.TmuxClient(run=Recorder())
    with pytest.raises(tmux.TmuxError):
        client.paste("%1", "")
    with pytest.raises(tmux.TmuxError):
        client.paste("%1", "a\x00b")
    with pytest.raises(tmux.TmuxError):
        client.paste("1; kill-server", "x")


def test_paste_refuses_escape_and_control_characters():
    recorder = Recorder()
    client = tmux.TmuxClient(run=recorder)
    for text in ("hi\x1b[201~rm -rf x\r", "a\x9b2Jb", "bell\x07", "del\x7f", "back\x08"):
        with pytest.raises(tmux.TmuxError, match="control character"):
            client.paste("%1", text)
    assert recorder.calls == []

def _capture(client, pane):
    done = subprocess.run(_with_socket(client, ["capture-pane", "-p", "-t", pane]), capture_output=True, text=True)
    return done.stdout


def _buffers(client):
    done = subprocess.run(_with_socket(client, ["list-buffers"]), capture_output=True, text=True)
    return done.stdout.strip()


def _with_socket(client, args):
    return [TMUX_PROGRAM, "-L", client.socket, "-f", "/dev/null", *args]


@needs_tmux
def test_set_remain_on_exit_turns_a_kept_window_into_a_dropped_one(tmp_path):
    with private_tmux() as client:
        client.ensure_session("demo-abc123", tmp_path, "events", ["sleep", "30"], {}, MARKER)
        window = client.new_window("demo-abc123", "engine", ["sleep", "2"], tmp_path, {})

        client.set_remain_on_exit(window, False)

        assert wait_until(lambda: window not in {w.id for w in client.windows("demo-abc123")})


def test_windows_skips_a_window_caught_while_tmux_destroys_it():
    """Between its last pane going and the window going, tmux still answers display-message for the window."""

    def run(argv, **kwargs):
        verb = argv[argv.index("-f") + 2] if "-f" in argv else argv[3]
        outputs = {"list-windows": ("@1\n@2\n", ""), "display-message": ("name\n", ""), "show-options": ("", ""),
                   "list-panes": ("%2 0\n", "")}
        if verb == "list-panes" and "@1" in argv:
            return SimpleNamespace(returncode=1, stdout="", stderr="can't find window: @1\n")
        out, err = outputs[verb]
        return SimpleNamespace(returncode=0, stdout=out, stderr=err)

    client = tmux.TmuxClient(socket="sock", run=run)
    assert [window.id for window in client.windows("demo-abc123")] == ["@2"]


def _fake_windows_client(listing: str) -> tmux.TmuxClient:
    def run(argv, **kwargs):
        verb = argv[argv.index("-f") + 2] if "-f" in argv else argv[3]
        outputs = {"list-windows": listing, "display-message": "name\n", "show-options": "", "list-panes": "%2 0\n"}
        return SimpleNamespace(returncode=0, stdout=outputs[verb], stderr="")

    return tmux.TmuxClient(socket="sock", run=run)


def test_windows_carries_the_window_activity_of_the_listing():
    client = _fake_windows_client("@1 1761735000\n@2 1761735100\n")
    assert [(window.id, window.activity) for window in client.windows("demo-abc123")] == [
        ("@1", 1761735000), ("@2", 1761735100)]


@pytest.mark.parametrize("listing", ["@1\n", "@1 \n", "@1 not-a-number\n"])
def test_windows_activity_is_none_when_the_listing_lacks_a_number(listing):
    window, = _fake_windows_client(listing).windows("demo-abc123")
    assert (window.id, window.activity) == ("@1", None)


def test_windows_asks_tmux_for_the_activity_in_the_one_listing():
    seen = []

    def run(argv, **kwargs):
        seen.append(argv)
        return SimpleNamespace(returncode=0, stdout="", stderr="")

    tmux.TmuxClient(socket="sock", run=run).windows("demo-abc123")
    assert seen[0][-1] == "#{window_id} #{window_activity}"


def test_set_remain_on_exit_refuses_a_session_name():
    with pytest.raises(tmux.TmuxError, match="window id"):
        tmux.TmuxClient().set_remain_on_exit("demo", False)


@needs_tmux
def test_socket_path_is_the_path_of_the_server_socket(tmp_path):
    with private_tmux() as client:
        client.ensure_session("demo-abc123", tmp_path, "events", ["sleep", "30"], {}, MARKER)
        assert client.socket_path().endswith("/" + client.socket)


@needs_tmux
def test_select_window_makes_it_current_and_refuses_bad_ids(tmp_path):
    with private_tmux() as client:
        client.ensure_session("demo-abc123", tmp_path, "events", ["sleep", "30"], {}, MARKER)
        other = client.new_window("demo-abc123", "lane-a", ["sleep", "30"], tmp_path, {})
        client.select_window(other)
        current = subprocess.run([TMUX_PROGRAM, "-L", client.socket, "-f", "/dev/null", "display-message", "-p", "-t",
                                  "=demo-abc123:", "#{window_id}"], capture_output=True, text=True, check=True)
        assert current.stdout.strip() == other
        with pytest.raises(tmux.TmuxError, match="refused window id"):
            client.select_window("lane-a")


@needs_tmux
def test_server_start_time_is_a_recent_epoch_second_and_refuses_bad_ids(tmp_path):
    import time

    with private_tmux() as client:
        client.ensure_session("demo-abc123", tmp_path, "events", ["sleep", "30"], {}, MARKER)
        pane = client.windows("demo-abc123")[0].panes[0].id
        assert 0 <= time.time() - client.server_start_time(pane) < 60
        with pytest.raises(tmux.TmuxError, match="refused pane id"):
            client.server_start_time("events")


@needs_tmux
@pytest.mark.parametrize("launch", ["session", "window", "split"])
def test_a_window_runs_with_the_PATH_it_was_given_not_the_one_tmux_was_started_with(tmp_path, launch):
    given = f"{tmp_path}/given-bin:/usr/bin:/bin"
    out = tmp_path / "path.txt"
    # A pane that exits before tmux sets its marker can take the session with it.
    script = f"import os; open({str(out)!r}, 'w').write(os.environ['PATH']); input()"
    command = [sys.executable, "-c", script]
    with private_tmux() as client:
        if launch == "session":
            client.ensure_session("demo-abc123", tmp_path, "events", command, {"PATH": given}, MARKER)
        else:
            client.ensure_session("demo-abc123", tmp_path, "events", ["sleep", "30"], {}, MARKER)
            window = client.new_window("demo-abc123", "w", command if launch == "window" else ["sleep", "30"], tmp_path,
                                       {"PATH": given} if launch == "window" else {})
            if launch == "split":
                client.split_window(window, command, tmp_path, {"PATH": given}, "stack")
        assert wait_until(lambda: out.exists() and out.stat().st_size > 0)
    assert out.read_text() == given
