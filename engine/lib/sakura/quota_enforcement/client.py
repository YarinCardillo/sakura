"""Bounded loopback HTTP access to the quota service."""

from __future__ import annotations

import json
from http.client import HTTPConnection, HTTPException
from urllib.parse import urlsplit

DEFAULT_URL = "http://127.0.0.1:8799"
TIMEOUT_SECONDS = 1
MAX_RESPONSE_BYTES = 1024 * 1024


class Advisor:
    """Read advice over HTTP without proxies, redirects or external hosts."""

    def __init__(self, url: str = DEFAULT_URL):
        """Validate a plain HTTP loopback origin; raise ValueError otherwise."""
        parts = urlsplit(url)
        if (parts.scheme != "http" or parts.hostname != "127.0.0.1" or parts.username
                or parts.password or parts.path not in {"", "/"} or parts.query or parts.fragment):
            raise ValueError("quota advisor must be an HTTP origin on 127.0.0.1")
        self._port = parts.port or 8799

    def advise(self, request: dict) -> dict:
        """POST the plan's request; raise ValueError for malformed advice."""
        result = self._request("POST", "/advise", request)
        validate_advice(result)
        return result

    def status(self) -> dict:
        """GET the current status snapshot without triggering any local probe."""
        return self._request("GET", "/status")

    def _request(self, method: str, path: str, body: dict | None = None) -> dict:
        connection = HTTPConnection("127.0.0.1", self._port, timeout=TIMEOUT_SECONDS)
        try:
            data = json.dumps(body).encode() if body is not None else None
            connection.request(method, path, body=data, headers={"Content-Type": "application/json"})
            response = connection.getresponse()
            raw = response.read(MAX_RESPONSE_BYTES + 1)
            if response.status != 200 or len(raw) > MAX_RESPONSE_BYTES:
                raise ValueError("quota advisor returned an unsuccessful or oversized response")
            result = json.loads(raw)
            if not isinstance(result, dict):
                raise ValueError("quota advisor response must be an object")
            return result
        except HTTPException as error:
            raise OSError("quota advisor HTTP failure") from error
        finally:
            connection.close()


def validate_advice(value: dict) -> None:
    """Validate the documented response, rejecting unknown verdicts and empty routing."""
    if value.get("verdict") not in {"go", "switch", "wait", "deny"}:
        raise ValueError("invalid quota verdict")
    if value.get("agent") not in {"claude", "codex"}:
        raise ValueError("invalid quota agent")
    keys = ("reason",) if value["verdict"] in {"deny", "wait"} else ("model", "effort", "config_dir", "reason")
    for key in keys:
        if not isinstance(value.get(key), str) or not value[key]:
            raise ValueError(f"invalid quota {key}")
    if value.get("retry_at") is not None and not isinstance(value["retry_at"], str):
        raise ValueError("invalid quota retry_at")
