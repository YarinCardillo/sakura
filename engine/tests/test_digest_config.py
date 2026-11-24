"""The digest's host file: where it is read from, who may own it and which sources it accepts."""

import os
import subprocess
from pathlib import Path

import pytest

from sakura import engine_home
from sakura.digest import config

HOME = Path("/home/someone")
MINIMAL = '[[source]]\nid = "work"\nrun = ["/opt/digest/work.sh", "demo/app"]\n'


def _write(path: Path, text: str, mode: int = 0o600) -> Path:
    path.write_text(text)
    os.chmod(path, mode)
    return path


def _source(**fields) -> dict:
    return {"source": [{"id": "work", "run": ["/opt/digest/work.sh"], **fields}]}


def test_Config_Defaults_AppliedToAMinimalFile(tmp_path):
    loaded = config.load(_write(tmp_path / "digest.toml", MINIMAL))
    assert (loaded.at, loaded.model, loaded.language, loaded.lookback_hours) == ("07:30", "sonnet", "en", 24)
    assert loaded.sources == (config.Source("work", run=("/opt/digest/work.sh", "demo/app")),)
    assert loaded.minute_of_day == 7 * 60 + 30


def test_Config_EngineHomeFile_IsTheDefault(tmp_path):
    _write(engine_home.root() / config.CONFIG_FILE, MINIMAL)
    assert config.load().sources[0].id == "work"


def test_Config_MissingFile_NamesWhereToCreateIt(tmp_path):
    with pytest.raises(config.ConfigError, match="create .*digest.toml"):
        config.load(tmp_path / "digest.toml")


def test_Config_RepositoryLayer_Refused(repo):
    path = _write(repo / "digest.toml", MINIMAL)
    with pytest.raises(config.ConfigError, match="repository"):
        config.load(path)


def test_Config_OpenToOthers_Refused(tmp_path):
    path = _write(tmp_path / "digest.toml", MINIMAL, mode=0o644)
    with pytest.raises(config.ConfigError, match="chmod 600"):
        config.load(path)


def test_Config_UnknownKey_Refused():
    with pytest.raises(config.ConfigError, match="unknown key 'when'"):
        config.parse({"when": "07:30"})
    with pytest.raises(config.ConfigError, match="unknown key 'token'"):
        config.parse(_source(token="x"))


def test_Config_SourceNeedsExactlyOneOfRunOrFeed():
    both = {"source": [{"id": "a", "run": ["/bin/true"], "feed": "https://example.org/a.atom"}]}
    neither = {"source": [{"id": "a"}]}
    for data in (both, neither):
        with pytest.raises(config.ConfigError, match="exactly one of run or feed"):
            config.parse(data)
    assert config.parse({"source": [{"id": "a", "feed": "https://example.org/a.atom"}]}).sources[0].feed


@pytest.mark.parametrize("bad", ["Work", "-work", "work_1", "", "a" * 33, "work space"])
def test_Config_SourceIdPattern_Enforced(bad):
    with pytest.raises(config.ConfigError, match="source id"):
        config.parse({"source": [{"id": bad, "run": ["/bin/true"]}]})


def test_Config_DuplicateSourceIds_Refused():
    entry = {"id": "a", "run": ["/bin/true"]}
    with pytest.raises(config.ConfigError, match="distinct"):
        config.parse({"source": [entry, entry]})


def test_Config_ProgramMustBeAbsolute_TildeUsesTheAccountHome():
    with pytest.raises(config.ConfigError, match="absolute program path"):
        config.parse(_source(run=["collect.sh"]))
    loaded = config.parse(_source(run=["~/digest/github.sh", "~/not-expanded"]), home=HOME)
    assert loaded.sources[0].run == ("/home/someone/digest/github.sh", "~/not-expanded")


@pytest.mark.parametrize("fields", [{"at": "7:30"}, {"at": "24:00"}, {"model": "Sonnet 4"}, {"language": "Italian"},
                                    {"lookback_hours": 0}, {"lookback_hours": 1000}, {"lookback_hours": True}])
def test_Config_BadScalar_Refused(fields):
    with pytest.raises(config.ConfigError):
        config.parse(fields)


@pytest.mark.parametrize("fields", [{"timeout_seconds": 0}, {"timeout_seconds": 100000}, {"run": []},
                                    {"run": [1]}, {"run": ["/bin/true"] * 65}, {"feed": "ftp://x"}])
def test_Config_BadSourceValue_Refused(fields):
    entry = {"id": "a", "run": ["/bin/true"], **fields}
    if "feed" in fields:
        del entry["run"]
    with pytest.raises(config.ConfigError):
        config.parse({"source": [entry]})


def test_Config_TooManySources_Refused():
    entries = [{"id": f"s{number}", "run": ["/bin/true"]} for number in range(config.MAX_SOURCES + 1)]
    with pytest.raises(config.ConfigError, match="limit"):
        config.parse({"source": entries})


# The Claude account the summarizer runs on

def _usage_toml(home: Path, text: str) -> None:
    path = home / ".config/sakura/usage.toml"
    path.parent.mkdir(parents=True)
    path.write_text(text)


def _known_account(monkeypatch, tmp_path) -> Path:
    home = tmp_path / "usage-home"
    monkeypatch.setenv("HOME", str(home))
    _usage_toml(home, f'[[sources]]\nprovider = "claude"\naccount = "work"\nroot = "{home}/.claude-work/projects"\n')
    return home


def test_Config_AccountMissing_MeansTheDefaultAccount():
    assert config.parse({}).account == ""
    assert config.parse(_source()).account == ""


def test_Config_AccountKnownToTheEngine_Accepted(monkeypatch, tmp_path):
    _known_account(monkeypatch, tmp_path)
    assert config.parse({"account": "work"}).account == "work"


def test_Config_AccountUnknown_RefusedWhenParsed(monkeypatch, tmp_path):
    _known_account(monkeypatch, tmp_path)
    with pytest.raises(config.ConfigError, match=r"account.*'ghost'.*work"):
        config.parse({"account": "ghost"})
    path = _write(tmp_path / "digest.toml", 'account = "ghost"\n' + MINIMAL)
    with pytest.raises(config.ConfigError, match="'ghost'"):
        config.load(path)


@pytest.mark.parametrize("bad", ["", 3, "two words", "a" * 70])
def test_Config_AccountNotAName_Refused(bad, monkeypatch, tmp_path):
    _known_account(monkeypatch, tmp_path)
    if bad == "":
        assert config.parse({"account": bad}).account == ""
        return
    with pytest.raises(config.ConfigError, match="account"):
        config.parse({"account": bad})


def test_Config_ASourceMayNameAConnectorOrBeSwitchedOff():
    loaded = config.parse({"source": [{"id": "code", "connector": "github"},
                                      {"id": "work", "run": ["/opt/digest/work.sh"], "enabled": False}]})
    assert loaded.sources[0] == config.Source("code", connector="github")
    assert loaded.sources[1].enabled is False and [source.id for source in loaded.active_sources] == ["code"]


@pytest.mark.parametrize("entry", [{"id": "a", "connector": "github", "feed": "https://example.com/feed"},
                                   {"id": "a", "connector": "Not Valid"}, {"id": "a", "run": ["/x"], "enabled": "no"}])
def test_Config_BadSourceKeys_Refused(entry):
    with pytest.raises(config.ConfigError):
        config.parse({"source": [entry]})


def test_Save_RoundTripsAndKeepsAPrivateBackupOfTheOldFile(tmp_path):
    original = config.parse({"at": "06:45", "account": "", "source": [
        {"id": "work", "run": ["/opt/digest/work.sh", "a b", 'say "hi"'], "timeout_seconds": 30},
        {"id": "news", "feed": "https://example.com/feed.xml"}, {"id": "code", "connector": "github", "enabled": False}]})
    path = tmp_path / "digest.toml"
    path.write_text("# my notes\n" + MINIMAL)
    path.chmod(0o600)
    config.save(original, path)
    assert config.load(path) == original
    assert (tmp_path / "digest.toml.bak").read_text().startswith("# my notes") and oct(path.stat().st_mode & 0o777) == "0o600"


# The Leeway accounts, as the app lists them, are accounts of the digest too

def _leeway_lists(monkeypatch, tmp_path, *accounts):
    from sakura.usage import leeway
    from leeway_support import make_helper

    monkeypatch.setenv(leeway.ENV_VARIABLE, str(make_helper(tmp_path / "helper", rows=list(accounts))))
    leeway.forget()


def test_Config_AccountListedByLeeway_AcceptedWhenTheRegistryKnowsNone(monkeypatch, tmp_path):
    from leeway_support import account

    monkeypatch.setenv("HOME", str(tmp_path / "nobody"))
    main = account("main")
    main[0]["config_dir"] = str(tmp_path / "main-dir")
    _leeway_lists(monkeypatch, tmp_path, main, account("cx", provider="codex"), account("old", hidden=True))
    assert config.parse({"account": "main"}).account == "main"
    for refused in ("cx", "old", "ghost"):
        with pytest.raises(config.ConfigError, match=f"'{refused}'"):
            config.parse({"account": refused})


def test_Config_AccountUnknownAndNoLeeway_KeepsTheRegistrysSentence(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path / "nobody"))
    with pytest.raises(config.ConfigError, match=r"account.*'main'.*known: none"):
        config.parse({"account": "main"})


def test_Config_ASourceNameAndWhatItReads_AreKeptAndWrittenBack(tmp_path):
    parsed = config.parse(_source(label="Work tickets", reads="Open tickets assigned to you"))
    assert (parsed.sources[0].label, parsed.sources[0].reads) == ("Work tickets", "Open tickets assigned to you")
    assert config.parse(__import__("tomllib").loads(config.dump(parsed))) == parsed


@pytest.mark.parametrize("fields", [{"label": 3}, {"label": "two\nlines"}, {"reads": "x" * 161}, {"label": "x" * 61}, {"reads": ""}])
def test_Config_ASourceNameOrReadsThatIsNotOneShortLine_IsRefused(fields):
    with pytest.raises(config.ConfigError, match="label|reads"):
        config.parse(_source(**fields))


@pytest.mark.parametrize("model", ["haiku", "claude:haiku", "codex", "codex:gpt-6.2-sol"])
def test_Config_TheModelIsAProviderAndModelLikeAnyRoleValue_ABareNameIsAClaudeModel(tmp_path, model):
    loaded = config.load(_write(tmp_path / "digest.toml", f'model = "{model}"\n{MINIMAL}'))
    assert loaded.model == model


@pytest.mark.parametrize("model", ["gemini:pro", "codex:", ":x", "codex:A B"])
def test_Config_AModelOfNoKnownProvider_IsRefused(tmp_path, model):
    with pytest.raises(config.ConfigError, match="model"):
        config.load(_write(tmp_path / "digest.toml", f'model = "{model}"\n{MINIMAL}'))
