"""The phone page, set up from the app: read its setup, list the overlay's devices of the same login, allow or remove a device,
set the port. cockpit.toml is written whole, atomically, mode 0600, and keeps what the engine does not know."""

import json
import stat
from pathlib import Path

import pytest

from sakura.cockpit import phone, settings

OWNER = "owner@example.com"
STATUS = {
    "BackendState": "Running",
    "Self": {"ID": "nMAC", "HostName": "owner-mac", "DNSName": "owner-mac.tail1234.ts.net.", "OS": "macOS", "UserID": 111,
             "TailscaleIPs": ["100.64.0.1", "fd7a:115c:a1e0::1"], "Online": True},
    "Peer": {
        "nodekey:phone": {"ID": "nPHONE", "HostName": "iPhone", "DNSName": "iphone.tail1234.ts.net.", "OS": "iOS",
                          "UserID": 111, "TailscaleIPs": ["100.64.0.2"], "Online": True},
        "nodekey:ipad": {"ID": "nIPAD", "HostName": "iPad", "DNSName": "ipad.tail1234.ts.net.", "OS": "iOS",
                         "UserID": 111, "TailscaleIPs": ["100.64.0.3"], "Online": False},
        "nodekey:friend": {"ID": "nFRIEND", "HostName": "friends-laptop", "DNSName": "friends-laptop.tail1234.ts.net.",
                           "OS": "linux", "UserID": 222, "TailscaleIPs": ["100.64.0.4"], "Online": True},
    },
    "User": {"111": {"ID": 111, "LoginName": OWNER, "DisplayName": "Owner"},
             "222": {"ID": 222, "LoginName": "friend@example.com", "DisplayName": "Friend"}},
}
LOGGED_OUT = {"BackendState": "NeedsLogin", "Peer": {}, "User": {}}


def _tailscale(directory: Path, document: dict | None = STATUS) -> Path:
    """A fake Tailscale CLI whose `status --json` prints `document`; the file is owner-only, as a trusted helper must be."""
    script = directory / "tailscale"
    script.write_text("#!/bin/sh\ncase \"$1\" in\n  status) cat <<'JSON'\n" + json.dumps(document) + "\nJSON\n;;\n"
                      "  *) exit 2;;\nesac\n")
    script.chmod(stat.S_IRWXU)
    return script


@pytest.fixture
def home(tmp_path):
    directory = tmp_path / "bin"
    directory.mkdir()
    return SimpleHome(tmp_path / "cockpit.toml", _tailscale(directory), directory)


class SimpleHome:
    def __init__(self, path, cli, directory):
        self.path, self.cli, self.directory = path, cli, directory

    def options(self):
        return {"path": self.path, "candidates": (str(self.cli),)}


def _read(home):
    return phone.read(**home.options())


# -- reading the setup

def test_Read_NoFileYet_ShowsTheMacItsAddressNameAndLinkAndNothingAllowed(home):
    shown = _read(home)

    assert shown["enabled"] is False and shown["port"] == 8787
    assert shown["tailscale"] == {"installed": True, "logged_in": True, "path": str(home.cli)}
    assert shown["mac"] == {"id": "nMAC", "name": "owner-mac.tail1234.ts.net", "address": "100.64.0.1", "login": OWNER}
    assert shown["link"] == "http://owner-mac.tail1234.ts.net:8787/"
    assert shown["allowed_logins"] == [] and shown["allowed_devices"] == []


def test_Read_ListsTheOverlayDevicesOfTheSameLoginOnlyWithWhetherEachIsAllowed(home):
    home.path.write_text(f'port = 8787\nallow_logins = ["{OWNER}"]\nallow_nodes = ["nPHONE"]\ntailscale = "{home.cli}"\n')

    shown = _read(home)

    assert [(d["id"], d["name"], d["os"], d["online"], d["allowed"], d["is_this_mac"]) for d in shown["devices"]] == [
        ("nMAC", "owner-mac", "macOS", True, False, True), ("nPHONE", "iPhone", "iOS", True, True, False),
        ("nIPAD", "iPad", "iOS", False, False, False)], "the friend's laptop is another login's"
    assert shown["enabled"] is True and shown["allowed_logins"] == [OWNER]
    assert shown["allowed_devices"] == [{"id": "nPHONE", "name": "iPhone", "login": OWNER, "known": True}]


def test_Read_AnAllowedDeviceThatIsNotOnTheOverlayAnyMore_IsKeptByIdWithNoName(home):
    home.path.write_text(f'allow_logins = ["{OWNER}"]\nallow_nodes = ["nGONE"]\ntailscale = "{home.cli}"\n')

    assert _read(home)["allowed_devices"] == [{"id": "nGONE", "name": "", "login": "", "known": False}]


def test_Read_ReadsTailscaleOnlyThroughStatusAndChangesNothing(home, tmp_path):
    log = tmp_path / "calls"
    home.cli.write_text(f'#!/bin/sh\necho "$@" >> "{log}"\ncat <<\'JSON\'\n{json.dumps(STATUS)}\nJSON\n')

    _read(home)
    phone.read(**home.options())

    assert set(log.read_text().split("\n")) <= {"status --json", ""}, "read only: nothing that changes Tailscale"


# -- allowing and removing a device

def test_Allow_FirstTime_WritesTheFileWithTheMacsLoginTheDeviceAndTheCliAtMode0600(home):
    phone.allow_device("nPHONE", **home.options())

    loaded = settings.load(home.path, registered_repos=[])
    assert loaded.allow_logins == frozenset({OWNER}) and loaded.allow_nodes == frozenset({"nPHONE"})
    assert loaded.tailscale == home.cli and loaded.port == 8787
    assert stat.S_IMODE(home.path.stat().st_mode) == 0o600


def test_Allow_ADeviceThatIsNotOnTheOverlay_IsRefusedAndWritesNothing(home):
    with pytest.raises(phone.PhoneSetupRefused, match="not on your overlay"):
        phone.allow_device("nSTRANGER", **home.options())

    assert not home.path.exists()


def test_Allow_ADeviceOfAnotherLogin_IsRefused(home):
    with pytest.raises(phone.PhoneSetupRefused, match="another login"):
        phone.allow_device("nFRIEND", **home.options())


def test_Allow_KeepsTheKeysItDoesNotKnowAndTheOtherDevices(home):
    home.path.write_text(f'port = 9000\nallow_logins = ["{OWNER}"]\nallow_nodes = ["nIPAD"]\ntailscale = "{home.cli}"\n'
                         'note = "kept"\n')

    phone.allow_device("nPHONE", **home.options())

    text = home.path.read_text()
    assert 'note = "kept"' in text and "9000" in text
    data = __import__("tomllib").loads(text)
    assert sorted(data["allow_nodes"]) == ["nIPAD", "nPHONE"] and data["allow_logins"] == [OWNER]


def test_Allow_ADeviceAlreadyAllowed_ChangesNothing(home):
    phone.allow_device("nPHONE", **home.options())
    before = home.path.read_text()

    phone.allow_device("nPHONE", **home.options())

    assert home.path.read_text() == before


def test_Remove_TakesTheDeviceOutAndKeepsTheRest(home):
    phone.allow_device("nPHONE", **home.options())
    phone.allow_device("nIPAD", **home.options())

    phone.remove_device("nPHONE", **home.options())

    assert settings.load(home.path, registered_repos=[]).allow_nodes == frozenset({"nIPAD"})


def test_Remove_ADeviceThatIsNotAllowed_IsRefused(home):
    phone.allow_device("nPHONE", **home.options())

    with pytest.raises(phone.PhoneSetupRefused, match="not allowed"):
        phone.remove_device("nIPAD", **home.options())


def test_Remove_AnAllowedDeviceThatLeftTheOverlay_CanStillBeRemoved(home):
    home.path.write_text(f'allow_logins = ["{OWNER}"]\nallow_nodes = ["nGONE"]\ntailscale = "{home.cli}"\n')

    phone.remove_device("nGONE", **home.options())

    assert settings.load(home.path, registered_repos=[]).allow_nodes == frozenset()


# -- the port

def test_SetPort_KeepsItInTheFileAndTheLink(home):
    phone.allow_device("nPHONE", **home.options())

    phone.set_port(8899, **home.options())

    assert settings.load(home.path, registered_repos=[]).port == 8899
    assert _read(home)["link"] == "http://owner-mac.tail1234.ts.net:8899/"


@pytest.mark.parametrize("port", [0, 70000, -1, "80x", True])
def test_SetPort_ANumberThatIsNoPort_IsRefusedAndKeepsTheOldOne(home, port):
    phone.allow_device("nPHONE", **home.options())

    with pytest.raises(phone.PhoneSetupRefused, match="port"):
        phone.set_port(port, **home.options())

    assert settings.load(home.path, registered_repos=[]).port == 8787


# -- writing

def test_Write_IsAtomicAndNeverLeavesAFileOthersCanRead(home):
    phone.allow_device("nPHONE", **home.options())

    assert [item.name for item in home.path.parent.iterdir() if item.name.startswith(".cockpit")] == []
    assert stat.S_IMODE(home.path.stat().st_mode) == 0o600


def test_Write_AFileThatCannotBeReadIsNeverOverwritten(home):
    home.path.write_text("port = [\n")

    with pytest.raises(phone.PhoneSetupRefused, match="cannot be read"):
        phone.allow_device("nPHONE", **home.options())

    assert home.path.read_text() == "port = [\n"


# -- `sakura cockpit phone`

@pytest.fixture
def cli_home(home, monkeypatch):
    monkeypatch.setattr(phone, "CLI_CANDIDATES", (str(home.cli),))
    monkeypatch.setattr(settings, "DEFAULT_PATH", str(home.path))
    return home


def _main(argv):
    from sakura.cockpit import cli

    return cli.main(argv)


def test_Cli_Phone_PrintsTheSetupAsJson(cli_home, capsys):
    assert _main(["phone", "--json"]) == 0

    shown = json.loads(capsys.readouterr().out)
    assert shown["link"] == "http://owner-mac.tail1234.ts.net:8787/" and shown["enabled"] is False


def test_Cli_Phone_AllowRemoveAndPortChangeTheFileAndPrintTheNewSetup(cli_home, capsys):
    assert _main(["phone", "--json", "allow", "nPHONE"]) == 0
    assert json.loads(capsys.readouterr().out)["allowed_devices"][0]["id"] == "nPHONE"
    assert _main(["phone", "--json", "port", "8899"]) == 0
    assert json.loads(capsys.readouterr().out)["port"] == 8899
    assert _main(["phone", "--json", "remove", "nPHONE"]) == 0
    assert json.loads(capsys.readouterr().out)["allowed_devices"] == []


def test_Cli_Phone_ARefusalIsAReasonAndAUsageExit(cli_home, capsys):
    assert _main(["phone", "--json", "allow", "nSTRANGER"]) == 2

    assert "not on your overlay" in json.loads(capsys.readouterr().out)["reason"]


def test_Cli_Phone_ChangesAreNeverMadeInsideAnEngineRole(cli_home, monkeypatch, capsys):
    monkeypatch.setenv("SAKURA_FLOW_ROLE", "builder")

    assert _main(["phone", "--json", "allow", "nPHONE"]) == 2
    assert not cli_home.path.exists()
    assert _main(["phone", "--json"]) == 0, "reading is allowed"


# -- the app's actions

def _argv(spawns):
    return [part for part in spawns.calls[0][0] if not part.startswith("--actor=")]


def test_Actions_AllowRemoveAndPortRunThePhoneCommandAndNeverComeFromARemoteDevice():
    from sakura.cockpit import actions, cli, host_actions
    from test_cockpit_actions import invoke

    _, allow = invoke("F", host_actions.ENGINE, {"device": "nPHONE"})
    _, remove = invoke("Z", host_actions.ENGINE, {"device": "nPHONE"})
    _, port = invoke("E", host_actions.ENGINE, {"port": "8899"})

    assert [_argv(spawns)[1:] for spawns in (allow, remove, port)] == [
        ["cockpit", "phone", "--json", "allow", "--", "nPHONE"], ["cockpit", "phone", "--json", "remove", "--", "nPHONE"],
        ["cockpit", "phone", "--json", "port", "--", "8899"]]
    parsed = cli.phone_parser().parse_args(_argv(allow)[3:])
    assert (parsed.change, parsed.value) == ("allow", "nPHONE")
    assert {"F", "Z", "E"} <= actions.LOCAL_ONLY_KEYS


@pytest.mark.parametrize("key,inputs,words", [
    ("F", {"device": "n;rm -rf"}, "invalid device"), ("F", {"device": "-x"}, "invalid device"),
    ("E", {"port": "eighty"}, "invalid port"), ("E", {"port": "70000"}, "invalid port")])
def test_Actions_AValueThatIsNoDeviceOrPort_IsRefusedBeforeAnyCommandRuns(key, inputs, words):
    from sakura.cockpit import actions, host_actions
    from test_cockpit_actions import OWNER

    with pytest.raises(actions.ActionRefused, match=words):
        actions.plan(key, host_actions.ENGINE, inputs, OWNER)


# -- a running serve takes the change in without a restart

PORT = 42005


def _running_server(tmp_path, helper, nodes=("nMAC",), port=PORT):
    import threading

    from sakura.cockpit import gate
    from sakura.cockpit.settings import CockpitSettings
    from test_cockpit_server import LOOPBACK, MemoryServer

    class Server(MemoryServer):
        resumed = threading.Event()
        stopped = threading.Event()

        def serve_forever(self):
            self.resumed.set()
            assert self.stopped.wait(10)

        def shutdown(self):
            self.stopped.set()

    own = gate.Self(LOOPBACK, "owner-mac.tail1234.ts.net", "nMAC")
    current = CockpitSettings(port, frozenset({OWNER}), frozenset(nodes), helper)
    return Server(own, current, lambda *a: "", lambda *a: "", access_log=tmp_path / "access.log"), current


def test_Reload_AnAllowedDeviceIsAdmittedAtOnceWithoutARestart(tmp_path, home):
    from sakura.cockpit import gate, server

    running, current = _running_server(tmp_path, home.cli)
    try:
        phone_peer = gate.Identity(OWNER, "nPHONE", "phone")
        with pytest.raises(gate.Denied, match="node nPHONE is not allowed"):
            gate.check(gate.Request("GET", "127.0.0.1", (("host", "x"),)), running.settings, frozenset({"x"}),
                       lambda peer: phone_peer, csrf_secret=b"k" * 32, now=0)
        home.path.write_text(settings_file(home.cli, ["nMAC", "nPHONE"], port=PORT))

        watcher = server.SettingsWatcher(running, lambda: settings.load(home.path, registered_repos=[]))

        assert watcher.check() == "updated"
        assert gate.check(gate.Request("GET", "127.0.0.1", (("host", "x"),)), running.settings, frozenset({"x"}),
                          lambda peer: phone_peer, csrf_secret=b"k" * 32, now=0) == phone_peer
    finally:
        running.server_close()


def settings_file(cli, nodes, port):
    return (f'port = {port}\nallow_logins = ["{OWNER}"]\nallow_nodes = {json.dumps(nodes)}\ntailscale = "{cli}"\n')


def test_Reload_ARemovedDeviceIsRefusedAtOnce(tmp_path, home):
    from sakura.cockpit import server

    running, _ = _running_server(tmp_path, home.cli, nodes=("nMAC", "nPHONE"))
    try:
        home.path.write_text(settings_file(home.cli, ["nMAC"], port=PORT))

        assert server.SettingsWatcher(running, lambda: settings.load(home.path, registered_repos=[])).check() == "updated"
        assert running.settings.allow_nodes == frozenset({"nMAC"})
    finally:
        running.server_close()


def test_Reload_ANewPort_AsksForTheServerToBindAgain(tmp_path, home):
    from sakura.cockpit import server

    running, _ = _running_server(tmp_path, home.cli)
    try:
        home.path.write_text(settings_file(home.cli, ["nMAC"], port=65001))

        watcher = server.SettingsWatcher(running, lambda: settings.load(home.path, registered_repos=[]))

        assert watcher.check() == "rebind" and watcher.settings.port == 65001
    finally:
        running.server_close()


def test_Reload_AFileThatIsBrokenForAMoment_KeepsTheSettingsTheServerHas(tmp_path, home):
    from sakura.cockpit import server

    running, before = _running_server(tmp_path, home.cli)
    try:
        home.path.write_text("port = [\n")

        assert server.SettingsWatcher(running, lambda: settings.load(home.path, registered_repos=[])).check() == "kept"
        assert running.settings == before
    finally:
        running.server_close()


def test_Reload_NothingChanged_IsSaid(tmp_path, home):
    from sakura.cockpit import server

    running, before = _running_server(tmp_path, home.cli)
    try:
        assert server.SettingsWatcher(running, lambda: before).check() == "same"
    finally:
        running.server_close()


def test_Serve_ANewPortInTheFile_StopsTheServerSoItBindsAgainWithTheNewSettings(tmp_path, home, monkeypatch):
    from dataclasses import replace

    from sakura.cockpit import server

    monkeypatch.setattr(server, "WATCH_SECONDS", 0)
    running, before = _running_server(tmp_path, home.cli)
    moved = replace(before, port=PORT + 1)

    def reload():
        assert running.resumed.wait(10)
        return moved

    answer = server._serve_until_done(running, None, reload)
    assert answer == moved and running.stopped.is_set()


def test_Serve_WithNoChange_KeepsServingUntilItIsStopped(tmp_path, home, monkeypatch):
    import threading
    from concurrent.futures import ThreadPoolExecutor

    from sakura.cockpit import server

    monkeypatch.setattr(server, "WATCH_SECONDS", 0)
    running, before = _running_server(tmp_path, home.cli)
    stop = threading.Event()
    reloaded = threading.Event()

    def reload():
        reloaded.set()
        return before

    with ThreadPoolExecutor(max_workers=1) as worker:
        active = worker.submit(server._serve_until_done, running, stop, reload)
        try:
            assert running.resumed.wait(10) and reloaded.wait(10)
            assert not running.stopped.is_set()
        finally:
            stop.set()
        assert active.result(timeout=10) is None and running.stopped.is_set()


# -- the state the app shows, in plain words

def test_State_TailscaleNotInstalled_SaysSoAndWhatToDo(tmp_path):
    shown = phone.read(path=tmp_path / "cockpit.toml", candidates=(str(tmp_path / "missing"),))

    assert shown["state"] == "not_installed" and shown["tailscale"]["installed"] is False
    assert shown["words"] == "Tailscale is not installed on this Mac. Install it from tailscale.com and sign in, then come back here."
    assert shown["link"] == "" and shown["devices"] == [] and shown["mac"] is None


def test_State_InstalledButSignedOut_SaysSoAndWhatToDo(tmp_path):
    directory = tmp_path / "bin"
    directory.mkdir()
    cli = _tailscale(directory, LOGGED_OUT)

    shown = phone.read(path=tmp_path / "cockpit.toml", candidates=(str(cli),))

    assert shown["state"] == "logged_out" and shown["tailscale"] == {"installed": True, "logged_in": False, "path": str(cli)}
    assert shown["words"] == "Tailscale is installed but you are not signed in. Open Tailscale and sign in, then come back here."
    assert shown["link"] == ""


def test_State_SignedInWithNoDeviceAllowedYet_AsksToPickOneOfTheListedDevices(home):
    shown = _read(home)

    assert shown["state"] == "no_device"
    assert shown["words"] == "No device may open the phone page yet. Pick your phone in the list; it must be signed in to Tailscale with this account."


def test_State_NoOtherDeviceOnTheOverlay_SaysToInstallTailscaleOnThePhone(tmp_path):
    directory = tmp_path / "bin"
    directory.mkdir()
    alone = {**STATUS, "Peer": {}}
    cli = _tailscale(directory, alone)

    shown = phone.read(path=tmp_path / "cockpit.toml", candidates=(str(cli),))

    assert shown["state"] == "no_device"
    assert shown["words"] == ("No device may open the phone page yet, and none of your other devices is on your overlay. Install "
                              "Tailscale on your phone and sign in with this account: it will appear in the list.")


def test_State_ADeviceIsAllowed_IsReadyWithTheLink(home):
    phone.allow_device("nPHONE", **home.options())

    shown = _read(home)

    assert shown["state"] == "ready"
    assert shown["words"] == "The phone page is ready: open http://owner-mac.tail1234.ts.net:8787/ on iPhone."


def test_State_SeveralDevicesAllowed_NamesThemAll(home):
    phone.allow_device("nPHONE", **home.options())
    phone.allow_device("nIPAD", **home.options())

    assert _read(home)["words"] == ("The phone page is ready: open http://owner-mac.tail1234.ts.net:8787/ on iPhone or iPad.")


def test_State_AllowedDevicesThatAreNoLongerOnTheOverlay_IsNotReady(home):
    home.path.write_text(f'allow_logins = ["{OWNER}"]\nallow_nodes = ["nGONE"]\ntailscale = "{home.cli}"\n')

    assert _read(home)["state"] == "no_device"


def test_Actions_ThePhoneKeysDoNotTakeTheKeyOfAnotherAction():
    from sakura.cockpit import actions

    assert [actions.ACTIONS[key].label for key in ("F", "Z", "E")] == [
        "Allow device on the phone page", "Remove device from the phone page", "Set phone page port"]
    assert "phone" not in actions.ACTIONS["p"].label.lower(), "p stays the account action it was"
