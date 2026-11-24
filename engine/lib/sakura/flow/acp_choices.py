"""Select and validate session model and effort options at the ACP boundary."""

from dataclasses import dataclass

MAX_MODELS = 64


@dataclass(frozen=True)
class Option:
    id: str
    current: str
    values: tuple[dict, ...]
    method: str = "session/set_config_option"
    value_key: str = "value"

    def parameters(self, session: str, value: str) -> dict:
        params = {"sessionId": session, self.value_key: value}
        if self.method == "session/set_config_option":
            params["configId"] = self.id
        return params


def _items(options: object) -> list[dict]:
    return [item for item in options if isinstance(item, dict)] if isinstance(options, list) else []


def _values(items: object) -> tuple[dict, ...]:
    found = {}
    for item in _items(items):
        for value in _items(item["options"]) if "options" in item else (item,):
            key = value.get("value")
            if isinstance(key, str) and 0 < len(key) <= 256 and key.isprintable():
                found.setdefault(key, value)
    return tuple(found.values())


class Choices:
    """Track the latest complete option state for a single session."""

    def __init__(self):
        self.session = ""
        self._config: list[dict] = []
        self._legacy: dict = {}
        self._legacy_current: dict[str, str] = {}
        self._early: dict[str, list[dict]] = {}

    def opened(self, session: str, result: dict) -> None:
        """Bind updates to the session declared by session setup."""
        self.session = session
        self._legacy = result
        self.accept(result)
        if session in self._early and "configOptions" not in result:
            self._config = self._early[session]
        self._early.clear()

    def accept(self, result: dict) -> None:
        """Replace the complete list, including removals after a model change."""
        if "configOptions" in result:
            self._config = _items(result["configOptions"])

    def handle(self, message: dict) -> bool:
        """Accept only configuration notifications for the session being opened or used."""
        if message.get("method") != "session/update" or "id" in message:
            return False
        params = message.get("params")
        if not isinstance(params, dict) or not isinstance(params.get("sessionId"), str):
            return False
        session = params["sessionId"]
        update = params.get("update")
        if not isinstance(update, dict) or update.get("sessionUpdate") != "config_option_update":
            return False
        if session == self.session:
            self.accept(update)
            return True
        elif not self.session and len(self._early) < MAX_MODELS and "configOptions" in update:
            self._early[session] = _items(update["configOptions"])
        return False

    def option(self, category: str) -> Option | None:
        """Prefer semantic ACP config options and recognize the legacy model and mode fields."""
        for item in self._config:
            if item.get("category") == category and item.get("type") == "select" and isinstance(item.get("id"), str):
                current = item.get("currentValue")
                return Option(item["id"], current if isinstance(current, str) else "", _values(item.get("options")))
        return self._legacy_option(category)

    def _legacy_option(self, category: str) -> Option | None:
        fields = {"model": ("models", "Model", "modelId", "session/set_model"),
                  "mode": ("modes", "Mode", "id", "session/set_mode")}.get(category)
        if fields is None:
            return None
        key, label, value_key, method = fields
        legacy = self._legacy.get(key)
        if not isinstance(legacy, dict):
            return None
        values = [{**item, "value": item.get(value_key)} for item in _items(legacy.get(f"available{label}s"))]
        current = self._legacy_current.get(category, legacy.get(f"current{label}Id"))
        return Option(category, current if isinstance(current, str) else "", _values(values), method,
                      value_key if category == "model" else "modeId")

    def select(self, request, category: str, value: str, *, default: str = "", allow_change: bool = True) -> None:
        """Confirm a pinned choice, failing closed if it cannot be applied to this session."""
        from sakura.flow.acp import AcpError

        if not value:
            return
        option = self.option(category)
        if option is None and value == default:
            return
        error = "effort_unavailable" if category == "thought_level" else f"{category}_unavailable"
        if option is None or value not in {item["value"] for item in option.values}:
            raise AcpError(error)
        if value == option.current:
            return
        if not allow_change:
            raise AcpError(error)
        self.accept(request(option.method, option.parameters(self.session, value)))
        if option.method != "session/set_config_option":
            self._legacy_current[category] = value
        confirmed = self.option(category)
        if confirmed is None or confirmed.current != value:
            raise AcpError(error)
