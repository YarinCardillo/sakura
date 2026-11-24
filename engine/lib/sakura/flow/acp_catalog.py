"""Read the selected account's models without sending an inference prompt."""

import tempfile
from contextlib import contextmanager, suppress
from dataclasses import dataclass
from pathlib import Path

from sakura import programs
from sakura.accounts import goose
from sakura.accounts.providers import GOOSE
from sakura.flow import acp, acp_choices, cage, goose_cage
from sakura.flow.model_catalog import CatalogError, ModelOption
from sakura.setup import harnesses

PROVIDER_LABELS = {"ollama": "Ollama", "openai": "OpenAI"}


def fetch(account, root: Path) -> tuple[ModelOption, ...]:
    """Open a disposable session and enumerate each model with its own effort options."""
    try:
        if not account.enabled:
            raise CatalogError("account_disabled")
        installed = harnesses.qualify(account.provider)
        with (tempfile.TemporaryDirectory(prefix="models-", dir=root) as directory,
              _connection(installed, account, Path(directory).resolve()) as (client, caps, cwd, fallback)):
            probe = Probe(client)
            probe.open(cwd)
            try:
                return probe.models(fallback)
            finally:
                probe.close(caps)
    except (programs.ProgramError, acp.AcpError, cage.CageError) as error:
        raise CatalogError(str(error)) from None
    except OSError:
        raise CatalogError("catalog_storage_unavailable") from None


@contextmanager
def _connection(installed, account, cwd: Path):
    if account.provider != GOOSE:
        with installed.connect(cwd, account=account.config_dir) as (client, caps):
            yield client, caps, cwd, None
        return
    upstream = account.goose_provider
    if goose.readiness(account)["readiness"] != "ready":
        raise CatalogError("goose_account_connection_check_required")
    provider, model = goose_cage.qualify(installed, upstream, "")
    model_root, _ = goose_cage.model_storage(cwd, "catalog")
    goose_cage._write_config(model_root, upstream, model)
    profile = cwd / "catalog.sb"
    profile.write_text(goose_cage.model_profile(Path(installed.adapter.path), cage.account_home(), model_root, (), provider))
    env = programs.environment("system", installed.argv[0], {})
    env.update(HOME=str(cage.account_home()), GOOSE_PATH_ROOT=str(model_root), TMPDIR=str(model_root / "tmp"),
               GOOSE_PROVIDER=upstream, GOOSE_MODE="approve", GOOSE_TELEMETRY_ENABLED="false")
    if upstream == "ollama":
        env[provider.host_variable] = f"http://{provider.host}:{provider.port}"
    argv = (cage.SEATBELT, "-f", str(profile), *installed.argv)
    cwd = model_root / "cwd"
    default = ModelOption(model, f"{model} ({PROVIDER_LABELS[upstream]})", is_default=True)
    with installed.connect(cwd, launch=(argv, env)) as (client, caps):
        yield client, caps, cwd, default


@dataclass
class Probe:
    """A bounded, permission-free ACP model-list session."""

    client: acp.Client

    def __post_init__(self):
        self.choices = acp_choices.Choices()

    def open(self, cwd: Path) -> None:
        response = self.request("session/new", {"cwd": str(cwd), "mcpServers": []})
        session = response.get("sessionId")
        if not acp.valid_session_id(session):
            raise acp.AcpError("invalid_adapter_session")
        self.choices.opened(session, response)

    def request(self, method: str, params: dict) -> dict:
        def handle(message):
            self.choices.handle(message)
            return acp.probe_message(message)

        return self.client.request(method, params, handle)

    def models(self, fallback: ModelOption | None) -> tuple[ModelOption, ...]:
        model = self.choices.option("model")
        if model is None or not model.values:
            if fallback is None:
                raise CatalogError("model_enumeration_unavailable")
            return (fallback,)
        if len(model.values) > acp_choices.MAX_MODELS:
            raise CatalogError("model_enumeration_limit")
        return tuple(self._model(item, model.current) for item in model.values)

    def _model(self, item: dict, default: str) -> ModelOption:
        value = item["value"]
        self.choices.select(self.request, "model", value)
        effort = self.choices.option("thought_level")
        values = tuple(entry["value"] for entry in effort.values) if effort else ()
        current = effort.current if effort and effort.current in values else ""
        return ModelOption(value, str(item.get("name") or value), str(item.get("description") or ""),
                           values, current, value == default, str(item.get("resolvedModel") or ""))

    def close(self, caps) -> None:
        if caps.close_session and self.choices.session:
            self.client.close_grace()
            with suppress(acp.AcpError, OSError):
                self.request("session/close", {"sessionId": self.choices.session})
