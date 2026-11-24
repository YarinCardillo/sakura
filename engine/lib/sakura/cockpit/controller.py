"""Turns requests into outcomes: the one path from a page, a JSON call or the command line to a run.

The caller is never trusted. A request names an action key and a target key; the target is looked up in a fresh
snapshot, so the repository an action runs in always comes from the engine's own view, never from the request.
`perform` decides confirm, proof and run for every face; the faces only draw its outcome.
"""

from __future__ import annotations

import json
import sys
import time
from collections.abc import Callable
from dataclasses import dataclass, replace
from pathlib import Path

from sakura.actors import Actor
from sakura.cockpit import (
    INPUT_PREFIX,
    account_actions,
    actions,
    host_actions,
    pages,
    proofs,
    proposal_actions,
    remote,
    track_actions,
    wait_actions,
)
from sakura.cockpit.actions import Action, ActionResult, Invocation, Target
from sakura.cockpit.gate import Denied, Identity
from sakura.cockpit.items import Item
from sakura.cockpit.snapshot import Snapshot
from sakura.connectors import contract as connector_contract
from sakura.connectors import kinds
from sakura.connectors import registry as connector_registry
from sakura.flow import waits
from sakura.flow.render import strip_controls
from sakura.scheduled import definition as scheduled_definition
from sakura.scheduled import dispatch as scheduled_dispatch
from sakura.scheduled import store as scheduled_store
from sakura.tracks.domain import Refusal as TrackRefusal

ACTOR_SOURCE = "overlay"
PROOF_SOURCE_OVERLAY, PROOF_SOURCE_CLI = "overlay", "cli"  # the transports; a proof is only valid on its own
FIELD_KEY, FIELD_TARGET, FIELD_PROOF, FIELD_CONFIRM = "key", "target", "proof", "confirm"
COMPOSE_KEYS = ("w", "r")  # the actions whose input is words the person types
TARGET_KEY_LENGTHS = (2, 3, wait_actions.KEY_LENGTH)
MAX_KEY = 40
NO_TARGET = "no such target; it may have changed since the page was drawn"
NO_LONGER_WAITING = "no longer waiting: the engine has moved on since the page was drawn; nothing was run"
REMOTE_REFUSAL = "this control is only available on the machine the cockpit runs on"
PROOF_STORE_REFUSAL = "cannot check the confirmation: the proof store is unusable on the cockpit host"


class _Malformed(Exception):
    """A request no page of ours produces: the reason is logged and the request denied."""


@dataclass(frozen=True)
class _Request:
    key: str
    target_key: tuple
    inputs: dict
    proof: str


@dataclass(frozen=True)
class Confirm:
    """The action asks first: `token` is the one-time proof to send back, valid until `expires` (epoch seconds)."""

    token: str
    expires: int
    action: Action
    target: Target
    invocation: Invocation
    inputs: dict
    display_args: dict[str, str] | None = None


@dataclass(frozen=True)
class Done:
    """The command ran; its exit code may still be non-zero."""

    result: ActionResult


@dataclass(frozen=True)
class Refused:
    """Nothing ran. `denied` marks a request no face produces or a proof that does not match."""

    reason: str
    denied: bool = False
    reason_code: str = ""
    reason_args: dict | None = None


Outcome = Confirm | Done | Refused


def overlay_actor(identity: Identity) -> Actor:
    """The actor of a request that passed the gate."""
    return Actor(ACTOR_SOURCE, identity.login, identity.node_id, identity.device)


class Controller:
    """Holds the secret and the snapshot sources; `run`, `log_path` and `store` are injection points for tests."""

    def __init__(self, secret: bytes, snapshot_source: Callable[[], Snapshot], fresh_snapshot: Callable[[], Snapshot],
                 *, run: Callable = actions.run_in_own_group, log_path: Path | None = None,
                 clock: Callable[[], float] = time.time, store: proofs.ProofStore | None = None) -> None:
        self._secret, self._cached, self._fresh = secret, snapshot_source, fresh_snapshot
        self.remote_ids = remote.Ids(secret)
        self._run, self._log_path, self._clock, self._store = run, log_path, clock, store

    def index(self, identity: Identity, csrf: str) -> str:
        """The overview, from the snapshot the server keeps for a few seconds."""
        view = remote.VIEW_IDS.set(self.remote_ids)
        try:
            return pages.index(self.snapshot(), identity, csrf)
        finally:
            remote.VIEW_IDS.reset(view)

    def phone_fields(self, fields: dict) -> dict:
        """Resolve phone references to internal keys before normal action dispatch."""
        found = dict(fields)
        if FIELD_TARGET in found:
            key = self.remote_ids.lookup(found[FIELD_TARGET])
            found[FIELD_TARGET] = json.dumps(key) if key else "[]"
        project_field = INPUT_PREFIX + "project"
        if found.get(project_field):
            project = self.remote_ids.lookup(found[project_field])
            found[project_field] = project[1] if project and project[0] == "project" else "unknown-project"
        return found

    def phone_act(self, fields: dict, identity: Identity, csrf: str) -> str:
        """Render the phone action without host commands, diagnostics or paths."""
        view = remote.VIEW_IDS.set(self.remote_ids)
        try:
            return self.act(self.phone_fields(fields), identity, csrf)
        except Denied:
            raise Denied(remote.REFUSED) from None
        finally:
            remote.VIEW_IDS.reset(view)

    def snapshot(self) -> Snapshot:
        """The snapshot the server keeps for a few seconds (pages and JSON read the same one)."""
        return self._cached()

    def fresh_snapshot(self) -> Snapshot:
        """A snapshot read now, not the few seconds old copy."""
        return self._fresh()

    def act(self, fields: dict, identity: Identity, csrf: str) -> str:
        """Answer a POST to /action with a confirmation page or a result page.

        Raises gate.Denied for a request no page of ours produces and for a proof that does not match. An action
        the table refuses is a page saying why.
        """
        composing = self._compose(fields, csrf)
        if composing is not None:
            return composing
        outcome = self.perform(fields, overlay_actor(identity), proof_field=FIELD_PROOF,
                               proof_source=PROOF_SOURCE_OVERLAY)
        if isinstance(outcome, Confirm):
            return pages.confirm(outcome.action, outcome.target, outcome.invocation, outcome.inputs, csrf, outcome.token)
        if isinstance(outcome, Done):
            if fields[FIELD_KEY] == "read" and outcome.result.exit_code == 0:
                return self._open_wait(fields[FIELD_TARGET], identity, csrf)
            result = remote.result(fields[FIELD_KEY], outcome.result) if remote.VIEW_IDS.get() else outcome.result
            return pages.result(result)
        if outcome.denied:
            raise Denied(outcome.reason)
        if (remote.VIEW_IDS.get() and outcome.reason not in remote.SAFE_REFUSALS
                and not (fields[FIELD_KEY] == track_actions.SETTLE and outcome.reason_code)):
            return pages.refused(remote.REFUSED)
        return pages.refused(outcome.reason)

    def _open_wait(self, target: str, identity: Identity, csrf: str) -> str:
        snapshot = self._fresh()
        key = _target_key(target)
        wait = self._find_wait(snapshot, key)
        return pages.wait_view(snapshot, wait, csrf) if wait else pages.index(snapshot, identity, csrf)

    def perform(self, fields: dict, actor: Actor, *, proof_field: str, proof_source: str) -> Outcome:
        """Decide a request: ask for confirmation, run the action, or refuse. Every face goes through here.

        `fields` are the form fields (`key`, `target`, `input.<name>` and the proof under `proof_field`); `actor`
        is who asked. `proof_source` names the transport (`PROOF_SOURCE_OVERLAY` or `PROOF_SOURCE_CLI`): proofs
        are issued and accepted per transport, never by what the actor claims to be. Never raises for a bad
        request: the outcome says why nothing ran.
        """
        try:
            request = self._parse(fields, proof_field, proof_source)
        except _Malformed as malformed:
            actions.log_refusal(self._log_path, str(fields.get(FIELD_KEY, "")), (), actor, str(malformed))
            return Refused(str(malformed), denied=True)
        if proof_source == PROOF_SOURCE_OVERLAY:
            if actions.is_local_only(request.key, request.target_key):
                actions.log_refusal(self._log_path, request.key, request.target_key, actor, REMOTE_REFUSAL)
                code = "local_only" if request.key in connector_contract.ACTIONS else ""
                return Refused(REMOTE_REFUSAL, reason_code=code)
            request = replace(request, inputs=actions.remote_inputs(request.key, request.inputs))
        try:
            target = self._resolve(request.target_key, action_key=request.key, inputs=request.inputs)
        except scheduled_store.Refused as refusal:
            return Refused(str(refusal))
        if target is None:
            reason = NO_LONGER_WAITING if request.target_key[0] == wait_actions.KIND_WAIT else NO_TARGET
            actions.log_refusal(self._log_path, request.key, request.target_key, actor, reason)
            return Refused(reason)
        try:
            return self._execute(request, target, actor, proof_source)
        except TrackRefusal as refusal:
            return _track_refusal(refusal)
        except actions.ActionRefused as refusal:
            return Refused(str(refusal))
        except proofs.ProofStoreError as error:
            print(f"cockpit: {strip_controls(str(error))}", file=sys.stderr)  # holds a home path: never sent to a client
            return Refused(PROOF_STORE_REFUSAL)

    def perform_allowed(self, key: str, target_key: tuple, inputs: dict, actor: Actor, allowed: frozenset[str]) -> Outcome:
        """Run the action `key` for a face that has its own allow-list and no person to confirm (the master's).

        The allow-list is that face's whole authority: a key outside it is refused as denied, and an allowed action
        runs without a confirmation proof. The target is still looked up in a fresh snapshot, so the repository an
        action runs in comes from the engine's own view. Never raises for a bad request.
        """
        if (key not in allowed or key in scheduled_definition.ACTIONS
                or not target_key or target_key[0] == scheduled_definition.TASK_KIND
                or actor.source == "scheduled"):
            reason = f"action {key[:MAX_KEY]!r} is not one of this face's actions"
            actions.log_refusal(self._log_path, key, target_key, actor, reason)
            return Refused(reason, denied=True)
        target = self._resolve(target_key)
        if target is None:
            actions.log_refusal(self._log_path, key, target_key, actor, NO_TARGET)
            return Refused(NO_TARGET)
        try:
            return self._run_action(_Request(key, target_key, inputs, ""), target, actor)
        except TrackRefusal as refusal:
            return _track_refusal(refusal)
        except actions.ActionRefused as refusal:
            return Refused(str(refusal))

    def _compose(self, fields: dict, csrf: str) -> str | None:
        """The page for typing an answer, when a page posted a text action with no text yet; else None.

        The page index has no text field (it refreshes itself), so a message (`w`) or a reply (`r`) without a
        choice or text asks for the words here. Only the HTML face does this: JSON and the command line refuse the
        missing input. A request that names no applicable action and target is left to `perform` to refuse.
        """
        if fields.get(FIELD_KEY) not in COMPOSE_KEYS or set(fields) != {FIELD_KEY, FIELD_TARGET}:
            return None
        try:
            target = self._resolve(_target_key(fields[FIELD_TARGET]))
        except _Malformed:
            return None
        action = actions.ACTIONS[fields[FIELD_KEY]]
        if target is None or action not in actions.applicable(target):
            return None
        return pages.compose(action, target, csrf)

    # -- request ------------------------------------------------------------------------------

    @staticmethod
    def _parse(fields: dict, proof_field: str, proof_source: str) -> _Request:
        extra = [name for name in fields if name not in (FIELD_KEY, FIELD_TARGET, proof_field)
                 and not name.startswith(INPUT_PREFIX)]
        missing = [name for name in (FIELD_KEY, FIELD_TARGET) if name not in fields]
        if extra or missing:
            raise _Malformed(f"unexpected form fields: {', '.join(extra)}" if extra
                             else f"missing form fields: {', '.join(missing)}")
        inputs = {name[len(INPUT_PREFIX):]: value for name, value in fields.items() if name.startswith(INPUT_PREFIX)}
        if proof_source == PROOF_SOURCE_OVERLAY:
            multiline = {"R": "note", track_actions.SETTLE: "answer"}.get(fields[FIELD_KEY])
            if isinstance(inputs.get(multiline), str):
                inputs[multiline] = inputs[multiline].replace("\r\n", "\n").replace("\r", "\n")
        key = fields[FIELD_KEY]
        action = actions.ACTIONS.get(key)
        keep_empty = track_actions.START_INPUTS if key == "L" else ()
        for name in (action.optional if action else ()):
            if inputs.get(name) == "" and name not in keep_empty:
                del inputs[name]
        return _Request(fields[FIELD_KEY], _target_key(fields[FIELD_TARGET]), inputs, fields.get(proof_field, ""))

    def _resolve(self, target_key: tuple, *, action_key: str = "", inputs: dict | None = None) -> Target | None:
        if target_key[0] == scheduled_definition.TASK_KIND:
            if len(target_key) != 3:
                raise scheduled_store.Refused("stale_task")
            storage = scheduled_store.Store()
            if (action_key == scheduled_definition.RUN and inputs is not None and set(inputs) == {"op"}
                    and (replay := scheduled_dispatch.replay_target(storage, *target_key[1:], inputs["op"]))):
                return replay
            task = storage.get(target_key[1])
            if task.key != target_key:
                raise scheduled_store.Refused("stale_task")
            return task
        snapshot = self._fresh()
        if target_key[0] in (track_actions.COLLECTION, track_actions.RECORD):
            return _track_target(snapshot, target_key)
        if target_key[0] == wait_actions.KIND_WAIT:
            wait = self._find_wait(snapshot, target_key)
            if wait is None and action_key == track_actions.SETTLE and track_actions.is_decision_wait(target_key):
                return _replay_wait(target_key)
            if wait is None and action_key == proposal_actions.START:
                wait = proposal_actions.replay_wait(json.dumps(target_key, separators=(",", ":")))
            if wait is None:
                return None
            return snapshot.wait_target(wait)
        targets = (*host_actions.TARGETS, *snapshot.items, *snapshot.groups, *map(account_actions.target_of, snapshot.accounts), *account_actions.leeway_targets(),
                   *map(snapshot.project_target, snapshot.projects))
        return next((target for target in targets if target.key == target_key), None)

    @staticmethod
    def _find_wait(snapshot: Snapshot, target_key: tuple):
        return next((wait for wait in snapshot.waits if wait_actions.target_of(wait).key == target_key), None)

    # -- execution ----------------------------------------------------------------------------

    def _execute(self, request: _Request, target: Target, actor: Actor, proof_source: str) -> Outcome:
        action = actions.ACTIONS.get(request.key)
        if action is not None and action.confirm:
            if not request.proof:
                return self._ask(action, request, target, actor, proof_source)
            if not self._proof_accepted(request, target, actor, proof_source):
                reason = "confirmation proof rejected (wrong, expired or already used)"
                actions.log_refusal(self._log_path, request.key, request.target_key, actor, reason)
                return Refused(reason, denied=True)
        return self._run_action(request, target, actor)

    def _run_action(self, request: _Request, target: Target, actor: Actor) -> Outcome:
        done = actions.run(request.key, target, request.inputs, actor, run=self._run, log_path=self._log_path)
        reason = actions.refusal(request.key, done)
        return Refused(reason) if reason else Done(actions.client_result(request.key, done))

    def _ask(self, action: Action, request: _Request, target: Target, actor: Actor, proof_source: str) -> Outcome:
        invocation = actions.plan(request.key, target, request.inputs, actor, log_path=self._log_path)
        display_args = None
        if request.key == "connectorreplace":
            try:
                display_args = _replacement_preview(request.inputs)
            except (ValueError, OSError) as error:
                refusal = connector_contract.client_reason(str(error))
                actions.log_refusal(self._log_path, request.key, request.target_key, actor, refusal)
                return Refused(refusal)
        proof = proofs.issue(self._secret, _identity(actor), request.key, request.target_key, _proof_inputs(request, target),
                             now=self._clock(), source=proof_source)
        return Confirm(proof, proofs.expires_at(proof), action, target, invocation, request.inputs, display_args)

    def _proof_accepted(self, request: _Request, target: Target, actor: Actor, proof_source: str) -> bool:
        return proofs.consume(self._secret, request.proof, _identity(actor), request.key, request.target_key,
                              _proof_inputs(request, target), now=self._clock(), source=proof_source, store=self._store)


def _replacement_preview(inputs: dict) -> dict[str, str]:
    return connector_registry.preview_replacement(
        kinds.Ports(), connector_id=inputs["name"], revision=int(inputs["revision"]),
        candidate=inputs["candidate"], candidate_revision=int(inputs["candidate_revision"]),
    )


def _proof_inputs(request: _Request, target: Target) -> dict:
    """The inputs a confirmation binds: for a resume or an approval from a task's own card, also the state revision
    the person saw and, for an approval, the gate's own revision (the one the command sends and the engine checks under
    the lock), so a task that moved on, a new candidate under the same gate or an edited spec refuses the old
    confirmation. An approval of a wait binds the revision in the wait's key instead."""
    if request.key in ("s", "C") and isinstance(target, wait_actions.WaitTarget) and target.lane is not None:
        return {**request.inputs, "lane_revision": target.lane.resume_revision}
    if request.key == "R":
        lane = target.lane if isinstance(target, wait_actions.WaitTarget) else target
        if isinstance(lane, Item):
            return {**request.inputs, "stopped_revision": lane.resume_revision}
    if request.key == "H" and hasattr(target, "wait"):
        return {**request.inputs, "workspace_head": target.wait.workspace_head, "engine_head": target.wait.engine_head}
    if request.key == "a" and isinstance(target, Item):
        return {**request.inputs, "stopped_revision": target.resume_revision,
                "approval_revision": target.approval_revision}
    return request.inputs


def _identity(actor: Actor) -> Identity:
    return Identity(actor.login, actor.node_id, actor.device)


def _target_key(value: str) -> tuple:
    try:
        key = json.loads(value)
    except (ValueError, RecursionError) as error:  # a deeply nested target is a request no page makes
        raise _Malformed("target is not JSON") from error
    if key == [track_actions.COLLECTION]:
        return (track_actions.COLLECTION,)
    if not isinstance(key, list) or len(key) not in TARGET_KEY_LENGTHS or not all(isinstance(p, str) for p in key):
        raise _Malformed("target is not a key of two, three or five strings")
    return tuple(key)


def _track_target(snapshot: Snapshot, key: tuple) -> track_actions.Target | None:
    view = snapshot.tracks
    if view is None or view.problem:
        return None
    projects = tuple(snapshot.projects)
    if key == (track_actions.COLLECTION,):
        return track_actions.Target(projects=projects, offered=(track_actions.NEW,))
    if key[0] == track_actions.RECORD and len(key) == 2:
        for row in view.rows:
            if row["id"] == key[1]:
                return track_actions.Target(key[1], projects, tuple(action["key"] for action in row["actions"]))
    return None


def _replay_wait(key: tuple) -> wait_actions.WaitTarget:
    identity = json.dumps(key, separators=(",", ":"))
    wait = waits.Wait(identity, waits.TRACK_DECISION, "", "", "", "", "", None, source=waits.TRACK_SOURCE)
    return wait_actions.WaitTarget(wait, is_replay_only=True)


def _track_refusal(refusal: TrackRefusal) -> Refused:
    args = {"labels": list(refusal.labels)} if refusal.labels else {}
    return Refused(str(refusal), reason_code=refusal.code, reason_args=args)
