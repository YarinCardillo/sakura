"""The agent of a step of the lane flow: its account, the model of that account's program and the effort the model takes.

The lists come from the programs (`flow/model_catalog.py`, read from the cache). A preset names a model per tier; a step set to
a specific model or effort is an edit of the rules layer, and choosing the preset again drops it. Every account is offered on every
step: no provider is kept for or from a role.
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass, field

from sakura.accounts.providers import ADAPTERS, GOOSE
from sakura.flow import config, model_catalog, presets, provider_fallback, roles

LEGACY_CLAUDE_MODELS = ("haiku", "sonnet", "opus")  # the aliases Claude Code has always taken
COMMON_EFFORTS = ("low", "medium", "high", "xhigh", "max", "ultra")  # when no list of the model is kept
STEP_ROLES = {"plan": "planner", "plan_review": "plan_reviewer", "build": "builder", "task_review": "code_reviewer",
              "final_review": "code_reviewer", "checks": "evaluator"}
TIER_OF_ROLE = {role: tier for tier, members in presets.TIERS.items() for role in members}


class AgentRefused(ValueError):
    """The value is not one the step's account and program accept; the message says what is."""


@dataclass(frozen=True)
class AgentAccount:
    """One account a step may run on, as the person knows it."""

    id: str
    label: str
    provider: str
    upstream_provider: str | None = None

    @property
    def key(self) -> tuple[str, str]:
        return self.provider, self.id


@dataclass(frozen=True)
class Agents:
    """The accounts of the registry and what their programs list, read once for a describe or a change."""

    accounts: tuple[AgentAccount, ...]
    catalogs: dict[tuple[str, str], model_catalog.Catalog | None]
    statuses: dict[tuple[str, str], model_catalog.Status] = field(default_factory=dict)

    def of(self, account_id: str, provider: str | None = None) -> AgentAccount | None:
        matches = [account for account in self.accounts if account.id == account_id
                   and (provider is None or account.provider == provider)]
        return matches[0] if len(matches) == 1 else None

    def allowed(self) -> list[AgentAccount]:
        """The accounts a step may run on: every one of the registry, whatever its provider."""
        return list(self.accounts)

    def models(self, node: str, provider: str, pinned: str = "") -> list[model_catalog.ModelOption]:
        """The models the program of `provider` lists for a step: its pinned account's, else every allowed account's, once each."""
        accounts = [account for account in self.allowed() if account.provider == provider
                    and (not pinned or pinned == presets.PROJECT_ACCOUNT or account.id == pinned)]
        if provider == GOOSE and len(accounts) != 1:
            return []
        found: dict[str, model_catalog.ModelOption] = {}
        for account in accounts:
            for model in (self.catalogs.get(account.key).models if self.catalogs.get(account.key) else ()):
                found.setdefault(model.id, model)
        return list(found.values())


def load(accounts: Sequence, ids: Sequence[str] | None = None) -> Agents:
    """The agents of `accounts` (registry accounts), or of the plain Claude account `ids` an older caller names."""
    if accounts is None:
        accounts = [AgentAccount(identifier, identifier, config.ROLE_PROVIDERS[0]) for identifier in ids or []]
    known = tuple(AgentAccount(item.id, getattr(item, "label", item.id), item.provider,
                               getattr(item, "goose_provider", None)) for item in accounts)
    kept = {(item.provider, item.id): model_catalog.read(item) for item in accounts if hasattr(item, "config_dir")}
    statuses = {(item.provider, item.id): model_catalog.status(item) for item in accounts if hasattr(item, "config_dir")}
    return Agents(known, kept, statuses)


def view(node: str, cfg: config.FlowConfig, agents: Agents) -> dict:
    """What a step runs on now: its account, program, model and effort, the default model of its tier, and what it may choose."""
    role = STEP_ROLES[node]
    spec = roles.parse_role(cfg.values["roles"][role])
    pinned = cfg.values["step_accounts"].get(node, presets.PROJECT_ACCOUNT)
    model = spec.model or _program_default(spec.provider, cfg, agents, node, pinned)
    option = next((item for item in agents.models(node, spec.provider, pinned) if model in (item.id, item.resolved)), None)
    own = cfg.values["efforts"].get(role, "")
    preset_spec = roles.parse_role(presets.resolve(cfg.values["preset"]).models[TIER_OF_ROLE[role]])
    return {"account": pinned, "provider": spec.provider, "model": model, "resolved": option.resolved if option else "",
            "effort": own or _default_effort(spec.provider, option, cfg), "effort_is_set": bool(own),
            "default_model": preset_spec.model or _program_default(preset_spec.provider, cfg, agents, node, pinned),
            "accounts": [{"id": account.id, "label": account.label, "provider": account.provider}
                         for account in agents.allowed()],
            "note": _fallback_words(spec.provider, role, pinned, agents), "account_label": _account_label(pinned, agents, spec.provider),
            **_models_status(spec.provider, pinned, agents)}


MISSING_ACCOUNT = "an account Leeway does not list"
STATE_ORDER = (model_catalog.KEPT, model_catalog.STALE, model_catalog.REFRESHING, model_catalog.UNAVAILABLE, model_catalog.MISSING)


def _account_label(pinned: str, agents: Agents, provider: str) -> str | None:
    """The name of the account the step is pinned to, never its id; None for the project's own."""
    if pinned == presets.PROJECT_ACCOUNT:
        return None
    found = agents.of(pinned, provider) or agents.of(pinned)
    return found.label if found else MISSING_ACCOUNT


def _models_status(provider: str, pinned: str, agents: Agents) -> dict:
    """`models_state` (kept, refreshing, unavailable or missing) and `models_note`, the words that say it, for the models the
    step chooses from: those of its pinned account, else the best state among the accounts of its program."""
    candidates = [account for account in agents.accounts if account.provider == provider
                  and (pinned in (presets.PROJECT_ACCOUNT, account.id))]
    found = [agents.statuses[account.key] for account in candidates if account.key in agents.statuses]
    best = min(found, key=lambda status: STATE_ORDER.index(status.state), default=model_catalog.Status(model_catalog.MISSING))
    return {"models_state": best.state, "models_note": best.note,
            **({"models_reason": best.reason} if best.reason else {})}


def _fallback_words(provider: str, role: str, pinned: str, agents: Agents) -> str:
    """Why the step runs on an account of another provider than its own (`flow/provider_fallback.py`); empty when it does not."""
    found = provider_fallback.substitute(provider, role, agents.accounts) if pinned == presets.PROJECT_ACCOUNT else None
    return found.words if found else ""


def model_choices(node: str, agents: Agents, pinned: str = "") -> list[str]:
    """`provider:model` for every model the allowed accounts' programs list; the long-standing values when none is kept."""
    role = STEP_ROLES[node]
    found = [f"{provider}:{model.id}" for provider in config.providers_for(role) if provider in ADAPTERS
             and any(account.provider == provider for account in agents.allowed())
             for model in agents.models(node, provider, pinned if provider == GOOSE else "")]
    if found:
        return found
    return [*(f"claude:{alias}" for alias in LEGACY_CLAUDE_MODELS),
            *(["codex"] if config.CODEX_PROVIDER in config.providers_for(role) else [])]


def efforts_of(node: str, cfg: config.FlowConfig, agents: Agents) -> list[str]:
    """The efforts the step's model takes: the list kept for it, else the usual ones."""
    shown = view(node, cfg, agents)
    option = next((item for item in agents.models(node, shown["provider"], shown["account"])
                   if shown["model"] in (item.id, item.resolved)), None)
    if shown["provider"] == GOOSE:
        return list(option.efforts) if option else []
    return list(option.efforts) if option and option.efforts else list(COMMON_EFFORTS)


def check_model(node: str, value: str, cfg: config.FlowConfig, agents: Agents) -> str:
    """`value` (`provider:model`) as a model the step may run: of a program its account runs, listed by that program."""
    role = STEP_ROLES[node]
    provider, _, model = str(value).partition(":")
    if provider not in config.providers_for(role) or provider not in ADAPTERS:
        raise AgentRefused(f"the model must be one of {', '.join(model_choices(node, agents))}; got {value!r}")
    pinned = cfg.values["step_accounts"].get(node, presets.PROJECT_ACCOUNT)
    account = agents.of(pinned, provider) or agents.of(pinned)
    if account is not None and account.provider != provider:
        raise AgentRefused(f"this step runs on the {account.label} account, a {account.provider.title()} account: choose a "
                           f"{account.provider.title()} model, or change the account first")
    listed = agents.models(node, provider, pinned)
    if provider == config.ROLE_PROVIDERS[0] and (not model or model in LEGACY_CLAUDE_MODELS):
        return value
    if provider in (config.CODEX_PROVIDER, GOOSE) and not model:
        return value
    if not listed:
        raise AgentRefused(f"no list of {provider.title()} models is kept yet: run sakura engine models refresh")
    if not any(model in (item.id, item.resolved) for item in listed):
        raise AgentRefused(f"{provider.title()} does not list {model!r}; it lists {', '.join(item.id for item in listed)}")
    return value


def check_effort(node: str, value: str, cfg: config.FlowConfig, agents: Agents) -> str:
    """`value` as an effort the step's model takes; "" or `default` is the dial's own."""
    if str(value) in ("", "default"):
        return ""
    allowed = efforts_of(node, cfg, agents)
    if str(value) not in allowed:
        raise AgentRefused(f"this model takes the efforts {', '.join(allowed)}; got {value!r}")
    return str(value)


def check_account(node: str, value: str, agents: Agents) -> AgentAccount | None:
    """The account `value` names, as one the step may run on (None for the project's own)."""
    if value == presets.PROJECT_ACCOUNT:
        return None
    account = agents.of(value)
    if account is None:
        raise AgentRefused(f"unknown account {value!r}; choose one of {', '.join([presets.PROJECT_ACCOUNT, *(a.id for a in agents.accounts)])}")
    return account


def companions(node: str, account: AgentAccount | None, cfg: config.FlowConfig, agents: Agents) -> dict:
    """The other keys an account change carries: a step moved to the other program takes the preset's model of its tier when
    that is of the new program, else the program's own default, and its own effort goes (it belonged to the old model)."""
    role = STEP_ROLES[node]
    current = roles.parse_role(cfg.values["roles"][role])
    previous = cfg.values["step_accounts"].get(node, presets.PROJECT_ACCOUNT)
    selected = account.id if account else presets.PROJECT_ACCOUNT
    if previous == selected:
        return {}
    if current.provider == GOOSE and (account is None or account.provider == GOOSE):
        return {f"roles.{role}": GOOSE, f"efforts.{role}": ""}
    if account is None or account.provider == current.provider:
        return {}
    tier_model = presets.resolve(cfg.values["preset"]).models[TIER_OF_ROLE[role]]
    program = tier_model if tier_model.partition(":")[0] == account.provider else account.provider
    return {f"roles.{role}": program, f"efforts.{role}": ""}


def effort_after_model(node: str, role: str, value: str, cfg: config.FlowConfig, agents: Agents) -> dict:
    """An effort the step chose stays only when the new model takes it."""
    own = cfg.values["efforts"].get(role, "")
    if not own:
        return {}
    provider, _, model = value.partition(":")
    pinned = cfg.values["step_accounts"].get(node, presets.PROJECT_ACCOUNT)
    listed = next((item for item in agents.models(node, provider, pinned) if model in (item.id, item.resolved)), None)
    if (listed is None and provider != GOOSE) or (listed and own in listed.efforts):
        return {}
    return {f"efforts.{role}": ""}


def _program_default(provider: str, cfg: config.FlowConfig, agents: Agents, node: str, pinned: str) -> str:
    if provider in (config.CODEX_PROVIDER, GOOSE):
        listed = agents.models(node, provider, pinned)
        return next((item.id for item in listed if item.is_default), listed[0].id if listed else "")
    return config.DIAL[cfg.values["dial"]].claude_model


def _default_effort(provider: str, option: model_catalog.ModelOption | None, cfg: config.FlowConfig) -> str:
    if provider == GOOSE:
        return option.default_effort if option else ""
    return config.DIAL[cfg.values["dial"]].effort
