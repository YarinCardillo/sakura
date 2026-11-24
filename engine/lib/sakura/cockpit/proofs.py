"""Signed one-time confirmation proofs: a second POST runs a confirmed action only with a matching unused one."""

from __future__ import annotations

import hmac
import hashlib
import json
import re
import secrets
import time

from sakura.cockpit.gate import Identity
from sakura.cockpit.proofstore import STATE_DIRECTORY, NONCE_FILE, SECRET_FILE, ProofStore, ProofStoreError

PROOF_TTL_SECONDS = 120
NONCE_BYTES = 16
PROOF_SHAPE = re.compile(r"[0-9]{1,12}\.[0-9a-f]{32}\.[0-9a-f]{64}")
DOMAIN = "sakura-cockpit-proof-v2"
OVERLAY_SOURCE = "overlay"

__all__ = ["NONCE_FILE", "PROOF_TTL_SECONDS", "ProofStore", "ProofStoreError", "SECRET_FILE", "STATE_DIRECTORY",
           "consume", "consumed_count", "expires_at", "issue", "load_secret"]


def load_secret() -> bytes:
    """The signing secret shared by every cockpit process of this user (created on first use)."""
    return ProofStore().secret()


def expires_at(proof: str) -> int:
    """The expiry (epoch seconds) a proof issued by `issue` carries."""
    return int(proof.split(".", 1)[0])


def issue(secret: bytes, identity: Identity, key: str, target_key: tuple, inputs: dict, *,
          now: float | None = None, source: str = OVERLAY_SOURCE) -> str:
    """Return a proof `<expiry>.<nonce>.<mac>` bound to the source, identity, action key, target key and inputs."""
    expiry = int((time.time() if now is None else now) + PROOF_TTL_SECONDS)
    nonce = secrets.token_hex(NONCE_BYTES)
    return f"{expiry}.{nonce}.{_mac(secret, source, identity, key, target_key, inputs, expiry, nonce)}"


def consume(secret: bytes, proof: str, identity: Identity, key: str, target_key: tuple, inputs: dict, *,
            now: float | None = None, source: str = OVERLAY_SOURCE, store: ProofStore | None = None) -> bool:
    """True once, across processes, for a proof issued for exactly this source, login, node, action, target and inputs.

    A proof that does not match, has expired or was used before is False, and a mismatch does not use it up.
    `store` defaults to the cockpit state under HOME. Raises ProofStoreError when the store cannot be trusted.
    """
    now = time.time() if now is None else now
    if not PROOF_SHAPE.fullmatch(proof):
        return False
    expiry, nonce, mac = proof.split(".")
    if not 0 <= int(expiry) - now <= PROOF_TTL_SECONDS:
        return False
    if not hmac.compare_digest(mac, _mac(secret, source, identity, key, target_key, inputs, int(expiry), nonce)):
        return False
    return (store or ProofStore()).use_once(nonce, int(expiry), now)


def consumed_count(store: ProofStore | None = None) -> int:
    """How many used proofs are still remembered (for tests)."""
    return (store or ProofStore()).count()


def _mac(secret: bytes, source: str, identity: Identity, key: str, target_key: tuple, inputs: dict, expiry: int,
         nonce: str) -> str:
    claims = {"domain": DOMAIN, "source": source, "login": identity.login, "node": identity.node_id, "key": key,
              "target": list(target_key), "inputs": inputs, "expiry": expiry, "nonce": nonce}
    message = json.dumps(claims, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()
    return hmac.new(secret, message, hashlib.sha256).hexdigest()
