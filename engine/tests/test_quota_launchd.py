import plistlib
from pathlib import Path

TEMPLATE = Path(__file__).resolve().parents[1] / "docs/launchd/sakura-quota.plist.template"


def rendered():
    text = TEMPLATE.read_text().replace("__SAKURA_BIN__", "/kit/bin/sakura").replace("__HOME__", "/home/me")
    return plistlib.loads(text.encode())


def test_Template_RendersToAValidPlistThatServesAndStaysUp():
    plist = rendered()
    assert plist["ProgramArguments"][:3] == ["/kit/bin/sakura", "quota", "serve"]
    assert plist["KeepAlive"] is True and plist["RunAtLoad"] is True


def test_Template_RefreshesEveryTenMinutes():
    arguments = rendered()["ProgramArguments"]
    assert arguments[arguments.index("--refresh-minutes") + 1] == "10"


def test_Template_LeavesNoPlaceholderBehind():
    text = TEMPLATE.read_text().replace("__SAKURA_BIN__", "x").replace("__HOME__", "x")
    assert "__" not in text


def test_ExampleConfig_IsAValidQuotaConfig(tmp_path):
    from sakura.quota import config

    loaded = config.load(TEMPLATE.parents[1] / "examples/quota.toml", tmp_path)
    assert loaded.projects[0].allowed.claude == ("work", "personal")
