"""A fake `leeway` helper for the tests: a script that answers `version`, `status`, `accounts` and `advise` from canned
documents and writes every call to a log, so no test reads the real Leeway or its data."""

import json
import stat
import sys
from datetime import UTC, datetime, timedelta
from pathlib import Path

SCRIPT = """#!{python}
import json, sys, time
canned = json.loads({canned!r})
args = sys.argv[1:]
code = 0 if args[:1] == ["version"] else canned.get("exit_calls", 0)
with open({log!r}, "a") as log:
    log.write(json.dumps(args) + "\\n")
if canned.get("broken") and args[:1] != ["version"]:
    print("not json at all")
    raise SystemExit(0)
if args[:1] == ["version"]:
    if canned["exit_version"]:
        print(json.dumps({{"schema_version": 1, "error": "this leeway is older than the data it would read; use the newer one"}}))
        raise SystemExit(canned["exit_version"])
    print(json.dumps(canned["version"]))
elif args[:1] == ["status"]:
    print(json.dumps(canned["status"]))
elif args[:1] == ["refresh"]:
    time.sleep(canned.get("refresh_seconds", 0))
    print(json.dumps(canned["status"]))
elif args[:1] == ["serve"]:
    # A poller: lives until stopped (bounded, so a failed test leaves nothing behind), or ends at once when told to.
    deadline = time.monotonic() + (0 if canned.get("serve_exits") else 120)
    while time.monotonic() < deadline:
        time.sleep(0.05)
elif args[:2] == ["accounts", "list"]:
    print(json.dumps({{"schema_version": 1, "accounts": canned["accounts"]}}))
elif args[:2] == ["accounts", "add"]:
    book = canned["accounts"]
    folder = args[2]
    provider = args[args.index("--provider") + 1] if "--provider" in args else "claude"
    if not any(row["provider"] == provider and row["config_dir"] == folder for row in canned["status"]["accounts"]):
        book.append({{"id": "added-" + str(len(book) + 1), "provider": provider, "label": folder.rsplit("/", 1)[-1],
                     "origin": "added", "hidden": False, "folder_exists": True, "order": len(book)}})
    print(json.dumps({{"schema_version": 1, "accounts": book}}))
elif args[:1] == ["accounts"]:
    book = canned["accounts"]
    verb, ident = args[1], args[2]
    if verb == "remove":
        book[:] = [row for row in book if row["id"] != ident]
    for row in book:
        if row["id"] == ident and verb == "rename":
            row["label"] = args[3]
        if row["id"] == ident and verb in ("hide", "show"):
            row["hidden"] = verb == "hide"
    print(json.dumps({{"schema_version": 1, "accounts": book}}))
elif args[:1] == ["advise"]:
    print(json.dumps(canned["advice"]))
else:
    raise SystemExit(2)
raise SystemExit(code)
"""


def account(account_id, provider="claude", week=40.0, five=10.0, forecast=80.0, label=None, stale=False, hidden=False,
            as_of=None, week_resets_at=None):
    """One account as `status --json` and `accounts list --json` describe it: a pair (status row, book row).

    The week resets three days from now unless `week_resets_at` says otherwise: a reset in the past would end the week the
    guard judges, so a fixed date would turn every test of the guard into a different test once it had passed."""
    week_resets_at = week_resets_at or (datetime.now(UTC) + timedelta(days=3)).isoformat(timespec="seconds")
    row = {"id": account_id, "label": label or account_id.title(), "provider": provider, "config_dir": "", "registered": True,
           "aliases": [], "plan": "max5x", "five_hour": {"pct": five, "resets_at": "2026-11-18T15:00:00Z"},
           "seven_day": {"pct": week, "resets_at": week_resets_at},
           "forecast": {"seven_day_pct_at_reset": forecast, "average_pct_at_reset": forecast, "recent_pct_at_reset": forecast,
                        "range_low": forecast, "range_high": forecast, "next_round_pct": week, "exhausts_at": None,
                        "confidence": "medium"},
           "source": "probe", "stale": stale, "error": None, "as_of": as_of or datetime.now(UTC).isoformat(timespec="seconds"), "target_low": 75,
           "target_high": 100, "status": "on_target", "hours_early": None, "severity": None}
    book = {"id": account_id, "provider": provider, "label": label or account_id.title(), "origin": "discovered",
            "hidden": hidden, "folder_exists": True, "order": 0}
    return row, book


def make_helper(folder: Path, name: str = "leeway", *, version="0.1.2", store_schema=2, status_schema=5,
                advice_schema=2, accounts_schema=1, rows=(), advice=None, exit_version=0, broken=False,
                exit_calls=0, status=None, serve_exits=False, refresh_seconds=0) -> Path:
    """Write the fake helper `folder/name` and return its path; its calls go to `<path>.log` (one JSON list per call)."""
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / name
    status = status or {"schema_version": 5, "generated_at": "2026-11-18T12:00:00Z", "poller": "none",
                        "accounts": [status_row for status_row, _ in rows if not _["hidden"]], "history": {}}
    canned = {"version": {"schema_version": 1, "version": version, "status_schema": status_schema,
                          "accounts_schema": accounts_schema, "advice_schema": advice_schema, "store_schema": store_schema},
              "exit_version": exit_version, "status": status, "accounts": [book for _, book in rows],
              "advice": advice or {"schema_version": 2, "verdict": "go", "agent": "claude", "account": "", "model": "",
                                   "effort": "", "config_dir": "", "reason": "", "retry_at": None, "headroom_pct": None,
                                   "target_used": 90},
              "broken": broken, "exit_calls": exit_calls, "serve_exits": serve_exits,
              "refresh_seconds": refresh_seconds}
    path.write_text(SCRIPT.format(python=sys.executable, canned=json.dumps(canned), log=str(path) + ".log"))
    path.chmod(stat.S_IRWXU)
    return path


def calls(helper: Path) -> list[list[str]]:
    """The argument lists the fake helper was run with, in order."""
    log = Path(str(helper) + ".log")
    return [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
