"""The cockpit's controls over the engine's background work: always-on services, automations, connectors, the digest's
settings and "give priority to my sessions".

Each function turns the validated inputs of one action into the words and positionals of one engine command (the
cockpit's action table adds the program, the actor and the working folder), or refuses with one sentence. Nothing here
runs anything, and no input ever becomes a program or an option.
"""

from __future__ import annotations

from sakura.connectors import contract, kinds

KEEPER, PHONE_PAGE = "keeper", "phone-page"
SERVICE_NAMES = (KEEPER, PHONE_PAGE)
YES, NO = "yes", "no"
# The keys the person changes on the machine itself: installing an agent or touching a stored sign-in is never done from
# a device of the overlay.
LOCAL_ONLY_KEYS = frozenset("BOoIJA") | contract.ACTIONS
# The inputs these actions take, as (name, option) pairs for `sakura cockpit act`.
INPUTS = (("on", "on"), ("kind", "kind"), ("address", "address"), ("at", "at"), ("lookback_hours", "lookback_hours"),
          ("language", "language"), ("account", "account"), ("revision", "revision"),
          ("candidate", "candidate"), ("candidate_revision", "candidate_revision"))
DIGEST_SETTING_INPUTS = ("at", "lookback_hours", "language", "model", "account")
Command = tuple[tuple[str, ...], list[str], list[str]]  # words, options, positionals


class Refusal(ValueError):
    """Raised, before anything runs, with the one sentence that says why the inputs are not acceptable."""


def service_command(inputs: dict) -> Command:
    """`B`: install or remove the keeper, or turn the phone page under the keeper on or off.

    Raises Refusal for a service that is not always-on."""
    name, is_on = inputs["name"], inputs["on"] == YES
    if name == KEEPER:
        return ("keeper", "install" if is_on else "uninstall"), [], []
    if name == PHONE_PAGE:
        return ("keeper", "phone-page"), [], ["on" if is_on else "off"]
    raise Refusal(f"{name!r} is not an always-on service; the services are {', '.join(SERVICE_NAMES)}")


def connect_command(inputs: dict) -> Command:
    """`O`: connect an account or source of `kind`; a feed or program also takes a `label` and an `address`."""
    kind = inputs["kind"]
    if kind not in kinds.KINDS:
        raise Refusal(f"{kind!r} is not a kind of connector; the kinds are {', '.join(kinds.KINDS)}")
    options = [f"--label={inputs['label']}"] if "label" in inputs else []
    options += [f"--target={inputs['address']}"] if "address" in inputs else []
    return ("connectors", "connect"), options, [kind]


def disconnect_command(inputs: dict) -> Command:
    """`o`: disconnect the connector `name`."""
    return ("connectors", "disconnect"), [], [inputs["name"]]


def digest_settings_command(inputs: dict) -> Command:
    """`I`: change the digest's settings; each given input becomes one option, and at least one is needed."""
    if not inputs:
        raise Refusal(f"give at least one of {', '.join(DIGEST_SETTING_INPUTS)}")
    return ("digest", "settings"), [f"--{name}={value}" for name, value in inputs.items()], []


def digest_source_command(inputs: dict) -> Command:
    """`J`: switch the digest source `name` on or off."""
    return ("digest", "source"), [], [inputs["name"], "on" if inputs["on"] == YES else "off"]


def automation_command(inputs: dict) -> Command:
    """`A`: switch the automation `name` on (install its agent) or off (unload it, keep the definition)."""
    return ("automations", "enable" if inputs["on"] == YES else "disable"), [], [inputs["name"]]


def yield_command(inputs: dict) -> Command:
    """`Y`: give priority to the person's own sessions, or not."""
    return ("yield",), [], ["on" if inputs["on"] == YES else "off"]
