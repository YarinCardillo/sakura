# Cockpit API

The cockpit exposes what the flow engine and the outside lanes are doing, and the controls that change it, as a small JSON API. A dashboard reads it to draw the engine and posts to it to act. There are two faces of the same controller:

- **HTTP**, served by `sakura cockpit serve` on the machine's overlay address (`/api/...`).
- **Command line**, for a dashboard on the same machine: `sakura cockpit snapshot --json` and `sakura cockpit act ... --json`.

Phone session and snapshot documents carry `api: 2`. Local CLI snapshots carry `api: 1`. A client checks the version before using the document.

The cockpit keeps a bounded lookup table for phone target references. Everything comes from the engine's logs and task files, and every action runs one engine command from one action table, in the repository of the target, with the actor written to the action log and the engine ledger.

## The gate (HTTP)

Every request, API or not, goes through one gate. A request the gate refuses is answered `403` with the body `forbidden` and a line in `cockpit/access.log` of the state folder (`~/.local/state/sakura`, or `<engine home>/state` when `SAKURA_ENGINE_HOME` is set); nothing runs. A request that is malformed, oversized or not a form is answered `400`, `411`, `413`, `415` or `431` before the gate, after reading at most 16 KiB of body.

1. **Overlay identity.** The peer address is resolved with `tailscale whois`. The login must be in `allow_logins` and the node's stable id in `allow_nodes` of `~/.config/sakura/cockpit.toml`. Empty lists admit nobody. A lookup that fails or hangs refuses.
2. **Host.** Exactly one `Host` header, equal to `<overlay ip>:<port>` or `<MagicDNS name>:<port>` (trailing dot removed, default port 8787).
3. **Origin (POST only).** An `Origin` of scheme `http` whose authority equals the request's `Host`. A missing Origin, `null`, another scheme or another authority is refused.
4. **CSRF (POST only).** A form field `csrf` holding a token for this login and node, valid for 1800 seconds. Get it from `GET /api/session`; a token for one node does not work from another.
5. **Not the host (action POSTs only).** `POST /action` and `POST /api/action`, the first step and the confirmed step alike, are refused when the node that asks is the node the cockpit runs on (its own `Self.ID` in `tailscale status --json`). Nothing tells the person's browser from an agent process on the same machine, so a write from the cockpit's own machine goes through `sakura cockpit act` (see below), which is refused inside an engine role session. The answer is a `refused` result (`200`, reason `writes from the cockpit's own machine go through sakura cockpit act`; the HTML form gets a page saying the same) and a `refused` row in the action log. Reads (the pages, `GET /api/session`, `GET /api/snapshot`) stay allowed from the host. The cockpit refuses to start when `tailscale status --json` has no usable `Self.ID`, since it could not tell the host from other nodes. A dashboard on the Mac that hosts the cockpit uses the local CLI snapshot and acts with the command line. Other allowed nodes read and act over HTTP.

The API is same-origin only: there are no CORS headers (no CORS preflight is answered, `OPTIONS` is a `405`), so a page served from another origin cannot read a response or pass the Origin check. A dashboard that wants to use the HTTP face must be served from the cockpit's own authority, or use the command line face on the same machine (which is also the only way to act from the cockpit's own machine, see 5).

Limits, all failing closed: 10 seconds to read a request (the request line, headers and body; the identity lookup, the snapshot and the running of an action are outside it, an action has its own timeout of 120 seconds; an action runs in a process group of its own, and when it times out the whole group is ended (SIGTERM, a short grace, SIGKILL) and reaped before the action returns, so no command outlives it), request line and headers at most 8192 bytes and 50 headers, body at most 16384 bytes, at most 8 requests handled at once (a ninth gets `503`), and at most 2 actions running at once: while two are running, a third `POST /action` or `POST /api/action` is answered at once with `{"result": "refused", "reason": "another action is running"}` (`200` for JSON, a `503` page for the HTML form), nothing is spawned, and the reads keep answering. Ask again when one has finished.

Every response carries `Content-Security-Policy`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`. JSON responses are `application/json; charset=utf-8`. Strings never contain terminal control characters; times are ISO 8601 in UTC (`2026-10-26T08:00:00Z`) or `null` when unknown.

## Routes

| Route | Purpose |
|-|-|
| `GET /api/session` | who the gate recognized, and a CSRF token |
| `GET /api/snapshot` | everything the cockpit shows |
| `POST /api/action` | run, or ask confirmation for, one action |

Other methods on these paths are `405`; any other path under `/api/` is `404`. The snapshot is at most 5 seconds old.

### GET /api/session

```
GET /api/session HTTP/1.1
Host: mac.tail1234.ts.net:8787
```

```json
{"api": 2, "csrf": "1793003478.9f2c...", "identity": {"login": "owner@example.com", "node_id": "nMAC", "device": "mac"}}
```

| Field | Type | Meaning |
|-|-|-|
| `api` | integer | API version, `2` over HTTP |
| `csrf` | string | token for the `csrf` field of a POST |
| `identity.login` | string | overlay login |
| `identity.node_id` | string | immutable node id |
| `identity.device` | string | device name, display only |

### GET /api/snapshot

```
GET /api/snapshot HTTP/1.1
Host: mac.tail1234.ts.net:8787
```

```json
{"api": 1, "mode": "day", "deferred": [], "load": {"slots_total": 4, "slots_in_use": 1, "queue": [], "loadavg": [1.2, 1.0, 0.9]},
 "usage": {"now": "2026-10-26T08:00:00Z", "providers": []}, "problems": [],
 "accounts": [{"provider": "claude", "id": "work", "key": "work", "label": "Work", "state": "ok", "enabled": true, "week_pct": 61.5,
               "week_resets_at": "2026-11-02T00:00:00Z", "five_hour_pct": 33.0, "reserve_week_pct": 25.0, "reserve_week_pct_own": null, "stop_at_pct": 75.0, "open_alerts": 1,
               "plans": [{"plan": "max5x", "since": "2026-10-20T08:30:00Z"}, {"plan": "pro", "since": "2026-01-15"}],
               "plan_choices": ["pro", "max5x", "max20x"],
               "target": "[\"account\",\"claude\",\"work\"]",
               "actions": [{"key": "l", "label": "Rename", "needs": ["label"], "optional": [], "confirm": false}]}],
 "waits": [{"id": "[\"wait\",\"/work/repo\",\"demo\",\"approval\",\"checks\"]", "key": ["wait", "/work/repo", "demo", "approval", "checks"],
            "kind": "approval", "subkind": "approval", "resolution": "open", "section": "needs_you",
             "group": {"id": "[\"/work/repo\",\"demo\"]", "label": "demo"}, "is_read": false, "is_snoozed": false, "snoozed_until": null,
             "repo": "/work/repo", "project": "repo", "task": "demo", "title": "Run this check?",
            "detail": "They prove the change works: each one runs on this Mac, in the lane's own workspace.\n\n- pytest -q", "lane_title": "", "since": "2026-10-26T06:10:00Z",
            "actions": [{"key": "a", "label": "Approve", "needs": [], "optional": ["remember"], "confirm": true}]}],
 "groups": [],
 "items": [{"id": "[\"flow\",\"/work/repo\",\"demo\"]", "key": ["flow", "/work/repo", "demo"], "kind": "flow",
            "repo": "/work/repo", "worktree": "/work/repo-demo", "branch": "feat/demo", "last_commit": "a1b2c3d add parser",
            "stack": "up", "agent_state": "gate", "pr": "#12", "models": {"builder": "claude:opus"},
            "pending": ["dial strong"], "since": "2026-10-26T06:10:00Z", "last_activity_at": "2026-10-26T07:42:15Z", "usage": {"uncached_input": 10, "cache_write": 0, "cache_read": 5, "output": 7,
            "total": 22, "cost_usd": null}, "next_command": "sakura flow approve demo spec --continue",
            "progress": {"step": "build", "phase": "Build", "done": 2, "total": 5, "unit": "tasks", "text": "Build, 2 of 5 tasks", "engine_phase": "build", "tasks_done": 2, "tasks_total": 5, "checks_done": 0, "checks_total": 9, "overall_done": 2, "overall_total": 14},
            "actions": [{"key": "a", "label": "Approve", "needs": [], "optional": [], "confirm": true}]}],
 "events": [{"source": "flow", "scope": "repo/demo", "at": null, "text": "awaiting approval"}]}
```

Top level:

| Field | Type | Meaning |
|-|-|-|
| `api` | integer | `1` |
| `mode` | string | `day` or `night`; at night nothing pages |
| `deferred` | array of objects | pages held back at night: `at` (time or null), `task`, `event`, `message` (strings) |
| `load` | object | `slots_total`, `slots_in_use` (integers), `queue` (array of `label`, `command`, `waiting_seconds`, `critical`), `loadavg` (three numbers) |
| `usage` | object | `now` (time) and `providers`, see below |
| `problems` | array of strings | why part of the picture is missing (a broken config file, for instance) |
| `accounts` | array of account | every account of the registry, see below |
| `waits` | array of wait | every point a flow task or an outside lane waits on from the person, across projects, oldest first |
| `groups` | array of group | always empty: the tmux lane groups are shelved, and the field stays so `api` stays 1 |
| `items` | array of item | every flow task and every open outside lane |
| `events` | array of event | the newest 50, oldest first: `source` (`flow`, `lane` or `outside`), `scope`, `at`, `text` (raw), `kind` (the tag of a ledger line, a lane command, or the kind of an outside lane's log line: `opened`, `row`, `asked`, `changed`, `closed`, `steer`, `mark`, `reply`) and `words` (a plain sentence for an outside lane's line, such as `Added: <title>` or `You wrote to the lane`, else null; an app shows it instead of `text`). An outside line also has `by`, the actor that wrote it. `kind` and `words` are optional for a client |
| `projects` | array of project | registered projects in registration order, including projects with missing folders or no lanes |
| `decided` | array of decided | the asks the person answered in the last seven days (at most 200), newest first, each with `lane_title` (the lane's title, empty when it has none; `task` is the lane's name, never to be shown) and `title` (the ask's title in words, also for an ask whose record is gone: a gate is never named), and the answer given: Inbox > Done reads it, because an answered ask leaves `waits` at once. Optional for a client: an older app that ignores it keeps working |
| `host` | object | this machine's own controls: `id` (`["host","digest"]` as compact JSON, the `target` of its actions), `key` and `actions` (the same shape as an item's; today `G`) |
| `engine` | object | the engine's background work, see "The engine's background work" below. Optional for a client: an older app that ignores it keeps working |
| `automations` | object | `items` (array of `name`, `description`, `schedule_text` such as `daily 07:30`, `every 15 minutes` or `always on`, `enabled`, `installed`, `last_run_at` time or null, `last_result` `ok`, `failed with exit code N` or null), `target` (`["host","automations"]`) and `actions` (`A`). `installed` is what the engine recorded when the person accepted or enabled it: the snapshot never looks at the agents folder. The keeper and the phone page are listed as `always on` (their switch is `B`); the digest's own agent is `morning-digest`. Optional for a client |
| `connectors` | array of connector | Saved connector rows. An absent field means the engine does not supply the section. An empty array means no connections, except when `connectors_problem` is present |
| `connectors_problem` | string | Optional. `the connectors cannot be read` when the saved registry is unreadable |
| `connector_controls` | object | Optional. `version` (currently `1`), `target` (`["host","connectors"]` as compact JSON text), and `actions` (standard descriptors with `key`, `label`, `needs`, `optional`, `confirm`). Available with an empty list. A client that lacks support for the advertised version hides these controls |
| `digest` | object | the morning digest's settings and sources: `at`, `lookback_hours`, `language`, `model`, `account`, `next_at`, `last_run_at`, `sources` (array of `id`, `label` (the name a person reads: the source's own `label`, else its connector's name, else the id as words), `reads` (what it reads, one plain phrase: its own `reads`, else what its connector kind reads or `New entries of <host>` for a feed, else an empty string), `kind` `program`, `feed` or `connector`, `connector` the connector id or null, `enabled`, `last_good_run` time or null), and the controls: `target` and `actions` (`I`, `J`) and `connect` (`target`, `actions` of `O`, `o`). With a file that cannot be read it holds the controls and `problem`, a fixed sentence with no path. Optional for a client |

`usage.providers[]`: `provider` (`claude` or `codex`), `period_start`, `resets_at`, `utilization_at` (times or null), `utilization` (number 0 to 1 or null), `projection` (`current`, `at_reset` numbers, `limit_at` time or null, `trend` string; or null), `totals` (usage or null), `agents` (array of `item` as array of strings, `agent` string, `usage`, `share_of_local_tokens` number). A usage object is `uncached_input`, `cache_write`, `cache_read`, `output`, `total` (integers) and `cost_usd` (number or null). Null means the figure is not readable from local logs.

`accounts[]`, ordered by `provider` then `id`: `provider` (`claude` or `codex`), `id` (a slug, unique within its provider), `key` (the snapshot key that alerts and quota rows are stored under: the bare id for `claude`, `codex:<id>` for `codex`; match alerts on it and never rebuild the prefix), `label` (display name; show it instead of the id, the id is what alerts and holds are stored under), `state` (`ok`, `missing` when the account's directory is gone, or `disabled`, the rule of `sakura accounts`), `enabled` (boolean), `week_pct` (number 0 to 100 or null, the weekly utilization of the latest quota snapshot), `week_resets_at` (time or null), `five_hour_pct` (number or null), `reserve_week_pct` (number or null: the effective reserve, the account's own, else the global `budget.reserve_week_pct`), `reserve_week_pct_own` (number or null: the account's own reserve, null when the global one applies), `stop_at_pct` (number or null: `100 - reserve_week_pct`, the weekly utilization at which flows and the pace alert stop using the account), `plans` (array of `{plan, since}`, newest first; `since` is a `YYYY-MM-DD` date when the period starts at midnight UTC, else an ISO time with a Z; no prices and no extra usage), `plan_choices` (the plan names action `p` accepts for this provider: `pro`, `max5x`, `max20x` for `claude`; `plus`, `prolite`, `pro` for `codex`), `open_alerts` (integer, budget alerts not yet resolved for this account), `target` (the id to send as `target` of `POST /api/action`, the key `["account", provider, id]` as compact JSON, as an item's `id` is) and `actions` (the account actions, the same shape as an item's, see below; empty when the host has no `accounts.toml`, that is on a zero-config or not yet migrated host, where there is nothing to write to). The numbers are null and `open_alerts` is 0 when the usage archive is absent, empty or busy. A host with no `accounts.toml` lists its `usage.toml` accounts when that file still holds them (not yet migrated), else the directories named by `$CLAUDE_CONFIG_DIR` / `$CODEX_HOME` and the default directory of each provider, each as `default` (`default-2`, and so on, for more of a provider). A registry file that cannot be read gives `accounts: []` and one line in `problems`. There is no directory path, no price or extra usage, and no credential in an account (`plans` holds plan names and start dates only).

`projects[]`: `id` (the key `["project", folder]` as compact JSON, the `target` of `L`), `key`, `name` (the folder's name), `folder` (the project's repository path), `folder_missing` (boolean, true if the registered directory is unavailable), `branch` (string, the branch the folder has checked out, read from its HEAD file; `""` on a detached head or a missing folder), `open_lanes` (integer, the flow lanes of the project that count against the ceiling) and `actions` (`L` and `W` when the folder is available). Registration stores the canonical project path and Git common directory, so removing a linked worktree alias leaves the project in place. The snapshot retains saved lanes and waits while the canonical folder is missing. Their IDs are unchanged. Actions that need the folder are omitted and a direct request is refused with `The project folder is missing. Restore it and try again.`

Item:

| Field | Type | Meaning |
|-|-|-|
| `id` | string | stable across snapshots; the `target` of an action (the key as compact JSON) |
| `key` | array of strings | `["flow", repo, task]` or `["outside", lane]` (`["lane", group, name]` belonged to the shelved tmux lanes and is no longer listed) |
| `kind` | string | `flow`, `lane`, `outside` (a lane opened by an agent session the engine did not start, see `docs/outside-agents.md`) or `digest` (a day's morning digest: not a project and not a lane the person started, so an app keeps it out of the project and lane lists and shows it in the inbox and on a reading page of its own; its `key` is still `["outside", "digest-<date>"]`, `title` is `Morning digest`, `date` is its day `YYYY-MM-DD`, `repo` is absent and it carries `reading`) |
| `repo` | string | repository path (an outside lane: the folder its agent works in) |
| `worktree` | string or null | worktree path |
| `folder_missing` | boolean | true when the flow item's project folder is missing |
| `branch`, `last_commit`, `stack`, `pr`, `next_command` | string | `""` when unknown; `stack` is `up` or `down` for a flow |
| `agent_state` | string | flow: `gate`, `blocked`, `working`, `waiting`, `stopped`, `cleaned`, `delivered` (a finished lane that delivered; its `delivery` object names the branch and head or the pull request) or `unknown` (finished with no delivery); lane: `working`, `waiting` or `unknown`; outside lane: `waiting` while it has an open row or question, else `unknown` |
| `models` | object | role (flow) or tool (lane) to model |
| `pending` | array of strings | queued operator commands, and a problem found reading the task |
| `usage` | object or null | consumption of this week's period |
| `since` | string or null | when the item entered its current state, a time. Flow: the time of the last history entry, so for a gate the moment it began waiting for approval. Lane: when its worktree was created (null where the host keeps no creation time) |
| `last_activity_at` | string or null | the newest sign of life (a time). Flow: the newest of the last history entry, the ledger file and the run files. Lane: the newer of the tmux window's last output and the last commit of its branch. Null when nothing is known |
| `progress` | object or null | flow: the lane counted in the four steps the page draws (`step` is `plan`, `build`, `checks` or `deliver`, `phase` its name `Plan`, `Build`, `Checks` or `Deliver`) and within the current one, in words a person knows: `done` of `total` `unit`, and `text` the whole phrase (`Build, 2 of 5 tasks`, `Checks, 3 of 9 passed`; Plan and Deliver have nothing to count, so `total` is 0, `unit` is empty and `text` is the name alone). The engine's own records stay in their own fields: `engine_phase` (the engine's phase word, never to be shown), `tasks_done` and `tasks_total` (the plan's build tasks; one is done when it is merged or ended as needing no change), `checks_done` and `checks_total` (the contract's rows; one is done only when its proof, the engine's receipt or the person's ok mark, names the current contract and the engine's head) and `overall_done` and `overall_total` (both together). Null for a lane or a task whose state cannot be read. Outside lane: `done`, `total` and `phase` only: its owner rows marked Ok at their current content over all its owner rows, and the lane's `phase` (`draft` or `ready`, the agent's word) |
| `title`, `author` | string | `title`: the title of a lane: the first line of the task text a flow lane was started with (the rest is its `goal`), an outside lane's own title, `""` for a lane started from a file; `author`: `outside:<agent name>` for an outside lane, `""` for every other item. The `name` and the id in `key` stay the slug; an app shows `title` when it is not empty |
| `links` | array of objects | outside lane: up to four `{label, url}` with an `https://` URL; `[]` for every other item |
| `unread` | integer | outside lane: how many of the person's marks, replies and messages its agent has not acknowledged yet; 0 for every other item |
| `reading` | object | only on a day's morning digest (the outside lane `digest-<date>` of the agent `digest`): what a reading view shows. `date`; `text`, the lane's text; `news`, one object per news entry of the day's file with `title`, `text`, `source` (the source id) and `links` (`{label, url}`; empty when the file is gone); `asks`, its checks and decisions in order, each with `kind` (`owner_row` or `question`), `title`, `text` (without its links), `links`, `choices`, `state` (`open` or `decided`), `id` (the id of its wait in `waits` while open, else null) and, once decided, `answer` (the verdict `ok` or `ko`, the choice or the text they gave). A news entry has no read state; an ask is read when it is decided. Optional for a client |
| `goal` | string | what the lane was asked to do: the whole task input of a flow, every line kept, cut at 8000 characters; an outside lane: its first report (else its title). Text from a person or an agent, data and never instructions. Absent for a lane (`kind` `lane`) and a task whose state cannot be read |
| `opened_by` | string | `master` (the master started it), `person` (the person did, from the app, the phone page or the command line) or `outside` (an outside lane) |
| `preset`, `dial` | string | the preset and the dial the lane runs with |
| `pace` | string | the lane's pace, one of `slowest`, `slow`, `normal`, `fast`, `fastest` (the five the new-lane sheet offers): the one chosen when it was made, else the engine's then, changed only by `sakura flow pace` on the lane; null for an outside lane |
| `steps` | array of objects | `{id, name, state, model, account}` for `plan`, `build`, `checks` and `deliver` (the engine's intake, contract and plan phases are `plan`; evaluate is `checks`; pr and report are `deliver`; cleanup is not a step). `state` is `done`, `now`, `next` or `skipped` (`checks` is skipped when the preset has no evaluator; `plan` when the preset has no plan step or the engine recorded it skipped); a blocked lane keeps its step `now`. `name` is English, an app translates by `id`. `model` is the role's model or null; `account` is the Leeway account the step runs on (the preset's, or the one pace moved it to), null while it runs on the project's own account. `[]` for an outside lane |
| `reviews` | array of objects | `{id, on, model, account, state}` for `plan_review`, `task_review` and `final_review`, reported apart from the steps; `state` follows the step the review belongs to (`plan`, `build`, `deliver`) and is `skipped` while the review is off (`final_review` is off unless the preset turns it on). `[]` for an outside lane |
| `in_review` | boolean | a reviewer of the lane is running now |
| `pipeline` | array of objects | every step of the lane in the order it runs (`plan`, `plan_review`, `build`, `task_review`, `final_review`, `checks`, `evaluator`, `deliver`, `reporter`) as `{id, name, state, on, can_toggle, reason}`: `state` is `done`, `running`, `waiting`, `next` or `off`; `on` says whether the lane runs it; `can_toggle` is false, with the `reason` in plain words, for a step the lane has reached, for `build` and `deliver` (always on) and for one the lane cannot run (a review with no reviewer model, the plan review without the plan). The header shows the steps with `on`; the Steps menu lists them all and sends the action `t` for the ones with `can_toggle`. Absent for an outside lane; a step that ran (its run is on the lane's record) is `on` for the lane whatever the preset says now; `waiting` is a step the lane is in while its status is awaiting approval, blocked, failed or stopped, with `note` saying so in words (empty for every other state), and `reason` is why it cannot be toggled |
| `current_step` | object or null | the step the lane is in now, the one source for a header and a step bar: `{id, name, waiting, note}` of the last `pipeline` step that runs or waits (`final_review` while the code reviewer works even though `progress.step` is still `build`); null when none does. Absent for an outside lane |
| `actions` | array of action | the controls that apply to this item |

Wait: one point a task waits on, from the engine's records only (task state, the gate it stopped at, the contract's owner rows). A request the cage denied is information in the task's log and report, never a wait.

| Field | Type | Meaning |
|-|-|-|
| `id` | string | stable across snapshots; the `target` of an action (the key as compact JSON) |
| `key` | array of strings | `["wait", repo, task, kind, name]`: `name` is the gate of an approval, the row id of an owner row, the id of a proposal and the digest of the text shown, `<id>@<digest>`. An outside wait is `["wait", "outside", lane, kind, "<row or question id>@<digest>"]`: the digest names the exact content the person saw |
| `kind` | string | API v1 Inbox kind: `question`, `approval`, `review`, `blocked` or `proposal`. Owner rows, automations and knowledge changes appear as approvals. Digest checks and questions appear as proposals |
| `subkind` | string | original wait kind, equal to `key[3]`, used to dispatch its actions. Knowledge changes retain `proposal` here |
| `repo`, `project`, `task` | string | repository path, the project's name, the task id (an outside wait: the agent's folder, its name, the lane id) |
| `lane_title` | string | the title of the lane the ask belongs to (`items[].title`), `""` when it has none or the ask is not a lane's; an app shows it beside or instead of `task`. For an ask of a morning digest `project` is `""`, `lane_title` is `Morning digest` and `digest` is its day (`YYYY-MM-DD`); `digest` is null on other asks |
| `title`, `detail` | string | what to do, and what the person needs to know, in plain words (`flow/ask_words.py`). The title of an approval says the question: `Read the spec and approve it`, `Run these 3 checks?` (the body is one sentence on what they prove, a blank line, then every command as a `- ` line), asked too when the checks are turned on during a lane or the commands changed after they were approved (the lane waits at the checks gate, an `approval` wait with `a` Approve; Approve lets the lane run them and go on, and a Resume without it asks again in the same words), `Start now on <account label>?` (Approve starts the held lane now; the body says why it is held and when Sakura starts it again on its own), `Start the build?`, `Delivery overlaps another lane`, `Deliver although the review still wants changes?`, `Use the strongest model for a task?`; a stuck lane is `This lane is stuck` with the engine's reason as the body, as a plain sentence (what failed and what to do, for example `GitHub CLI is not logged in: run gh auth login in a terminal, then resume the lane.`, with no path of this Mac in it); when a reviewer's verdict stopped the lane (the contract's checks, the plan, a build task) the body is a lead sentence and the reviewer's own reason in at most two sentences, with check ids kept and folder paths, run names and task ids omitted, and the engine's full line is in `technical`; an owner row is `Try it yourself: <its first line>` |
| `technical` | string | the engine's own line when `detail` is a sentence made from it (a failed lookup, a command, a path of this Mac), for a view that shows technical detail on request; empty when `detail` is the line. Optional for a client |
| `findings` | array of strings | the reviewer's findings, one plain line each (project file names and check ids kept, host folders, run names and task ids omitted), on the ask `Deliver although the review still wants changes?` and on the stop of a lane whose final-review fix builder ended without changing anything; `[]` on every other ask. Optional for a client |
| `links` | array of `{label, url}` | titled https links an outside agent wrote in its text (`label: https://...`); `detail` and `title` do not repeat them, and a row with one step shows it as a sentence, not as `1.`. A review includes its pull request link. Optional for a client |
| `since` | string or null | when the task entered the state that waits, a time (an outside wait: when its row or question got its current content) |
| `source` | string | `flow` for the engine's own waits, `outside` for an outside agent's |
| `author` | string | outside wait: `outside:<agent name>`, shown as "From <agent> in <project>"; `""` for a flow wait |
| `choices` | array of strings | outside question: up to six answers to press; `[]` otherwise |
| `group` | object | `{id, label}`, both strings. A group identifies the lane or source |
| `section` | string | `needs_you` or `triage`. Proposal waits belong to Triage |
| `resolution` | string | `open` for all waits. An item that resolves moves to `decided` |
| `is_read` | boolean | whether the person marked this exact wait read |
| `is_snoozed` | boolean | the deadline is still in the future when the snapshot is collected. `waits` includes these rows. Use `true` for the Snoozed view and `false` for To do, Inbox counts, notifications and the Dock badge. The phone page groups snoozed waits in a Snoozed section with Unsnooze controls |
| `snoozed_until` | string or null | UTC ISO 8601 deadline. It can remain after the wait wakes. `unsnooze` clears it |
| `proposal` | object, absent on other kinds | start defaults for Triage, `{op, project, preset, account, title}`. Both surfaces submit the same `op` for the exact wait id. `project` is a registered repository folder, or null when a project needs to be chosen. `preset` and `account` are strings or null. `title` is a string |
| `folder_missing` | boolean | true when the wait's project folder is missing. Inbox attention actions and marking a saved pull request review reviewed still work |
| `wants_note` | boolean | true when a blocked lane asks for guidance on what to prove. The Resume action can carry that guidance in `note` |
| `actions` | array of action | every wait offers `read`, `unread` (no inputs) and `snooze` (requires `until`, a future offset-aware ISO timestamp or `1h`, `tomorrow` at local midnight, or `nextweek` at next Monday's local midnight). A snoozed wait also offers `unsnooze` (no inputs or confirmation), which clears the deadline and preserves its read mark. These actions never resolve a wait. Resolution actions: `a` for an approval, `k` for an owner row, `H` when its workspace head can be restored, `r` for a question, `y` and `N` for a knowledge change or an automation, `markreviewed` (no inputs or confirmation) for a review, `start` and `dismiss` for a Triage proposal, and for a blocked lane, applicable `R` (resume), `s` (stop) and `C` (discard failed work), each using the usual confirmation |

When lane state, configuration, the contract or the question log cannot be read, the Inbox shows a fixed message. The underlying error goes to the host log. Setup also keeps preset load errors in the host log.

The id of a wait is a valid `target`. A resolved wait leaves the next snapshot. Retrying a successful proposal `start` returns its existing lane id. Other actions on a resolved wait return `refused` with a reason starting `no longer waiting`.

Decided: one answer the person gave to an ask.

| Field | Type | Meaning |
|-|-|-|
| `id`, `key` | string, array of strings | the id and key the ask had as a wait (`["wait", repo, task, kind, name]`), so a row can be matched to the lane or project it came from |
| `kind` | string | the kind in the original wait key, including `review` |
| `project`, `task` | string | the project's name and the task id |
| `title` | string | what the ask said (written by an agent: shown as data); `""` when the engine could no longer read it |
| `answer` | string | `approve`, `decline` (reserved: no action gives it yet), `reply`, `choice`, `resume`, `stop`, `accept`, `reject`, `mark_ok`, `mark_ko`, `reviewed`, `merged`, `closed`, `started`, `dismissed` or `close` |
| `choice`, `text` | string or null | the answer pressed and the words typed (a reply, or the note of a mark), at most 4000 characters; null when there was none |
| `actor` | string | who resolved the wait (`app:<login>`, `overlay:<login>:<node>`, `cli:<user>` or `github`) |
| `place` | string | `desktop`, `phone`, `cli` or `github`. Earlier entries derive this from `actor`. Duplicate answer paths retain the first actor and place |
| `at` | string | when, a time |
| `lane` | string or null | the new lane id when the answer is `started` |

Pull request reviews come from local delivery records. The keeper checks up to four unresolved reviews in each background round and records explicit GitHub merge or close results. An unavailable or unknown remote state leaves the review visible.

Attention actions update only the addressed wait. The keeper prunes marks after 30 days and retains active snoozes.

On a wait whose Inbox `kind` is `proposal`, `start` requires `op` and accepts optional `project`, `preset`, `account` and `title`. The project must be a registered repository. If omitted, the engine uses the proposal's project or the sole registered project. Successful `output` is the created lane id. Retrying uses the same lane, including after either surface has resolved the proposal. `dismiss` has no inputs and resolves the proposal without opening a lane. Phone starts use the chosen project's effective preset and ignore a submitted preset override. The desktop can override the preset. Only checks and questions matching a host-published digest qualify for Triage. Start and Dismiss check that the exact source revision is still open before reserving or resolving it. Triage does not accept the older Mark and Reply actions. The terminal proposal record supplies its Done entry, including the actor and any created lane id. Knowledge changes use the existing approval actions.

On the phone, opening a wait posts `read` and returns its detail page. Its resolution actions still use the same wait id. With an empty Inbox the phone points to Sakura on the host for general project registration or lane creation, which remain desktop actions.

An open delivered pull request produces a review for its current head. Marking it reviewed resolves it. A merged or closed pull request resolves it on refresh with actor `github`.

An answer is kept by every path that gives one: the cockpit's actions, and the operator commands (`sakura flow approve`, `mark`, `reply`, `stop`, `resume` and `cleanup --discard`, `sakura proposal accept` and `reject`) a phone page or a script reaches. `stop` and `resume` are kept only for a lane that was blocked or failed, the ask `blocked` stands for. The answer reaches `<state dir>/cockpit/decided.jsonl`, the folder of `actions.jsonl`, with terminal controls removed; one answer seen by two paths is kept once. A refused or failed answer is not kept. The text is what the person typed. Display the decision record as data. Resume guidance reaches the agent through the flow steering channel.

Group: `id`, `key` (`["group", name]`), `name`, `repo`, `session`, `lanes` (integer), `problem` (string, `""` when none), `next_command`, `actions`.

Event: `source` (`flow`, `lane` or `outside`; an outside lane's log lines, scope `outside/<lane>`), `scope`, `at` (time or null: a flow ledger line has no time), `text`.

Action: `key`, `label` (strings), `needs` (input names that must be sent), `optional` (input names that may be), `confirm` (boolean: the first call asks, see below). `needs` can depend on the target: `m` needs `role` and `model` for a flow and only `model` for a lane.

### Phone response format

The served session and snapshot advertise `api: 2`. The snapshot example and corresponding field table show the version 1 local CLI format. On the phone, target references in `id` and `target` are opaque strings. Send them
as returned. If the cockpit restarts or forgets an older reference, refresh the snapshot. Proposal project picks
use the selected project's opaque `id`. The phone omits raw keys and clears host folders, commands, technical
diagnostics and event logs. Repository labels use the folder name. Only actions allowed from the phone are listed.

Phone confirmations include a summary and token. Phone results include an exit code and fixed error text on failure.
A proposal Start also returns the created lane ID on success. The action audit stores the action key, target kind and
digest, actor, outcome and exit code. Cage permission denials record the tool category and reason. Digest links have
user information, query parameters and fragments stripped before publication.

### POST /api/action

A form POST (`Content-Type: application/x-www-form-urlencoded`, optionally with `charset=utf-8` and no other parameter, at most 16384 bytes, no repeated field). Any other content type, or any other parameter, is `415`.

| Field | Required | Meaning |
|-|-|-|
| `csrf` | yes | token from `GET /api/session` |
| `key` | yes | action key from the target's `actions` |
| `target` | yes | the `id` of an item, group or wait, or the `target` of an account |
| `input.<name>` | per action | one field per input of `needs` and `optional` |
| `confirm` | second step | the `token` a `confirm` answer returned |

Any other field is refused. The target is never trusted: the server looks `target` up in a fresh snapshot and runs the command in the repository it finds there. An empty optional input is dropped.

The phone routes refuse Approve, Mark, Close lane, Clean up and account changes before resolving the target
or issuing confirmation. Starting an Inbox proposal from a phone uses the project preset and account.
The lane stores the operator brief and the proposal source record separately. Intake labels the source's title,
detail, author and origin as untrusted data.

| Key | Label | Applies to | Inputs | Confirm | Command |
|-|-|-|-|-|-|
| `d` | Close lane | lane, outside lane | none | yes | `sakura lane done`; outside lane: closes it as dropped |
| `m` | Change model | flow, lane | flow: `role`, `model`; lane: `model` | no | `sakura flow role` / `sakura lane model` |
| `D` | Change dial | flow | `step` | no | `sakura flow dial` |
| `a` | Approve | flow, approval wait | optional `remember` (`yes`, the `checks` gate only) | yes | `sakura flow approve ... --continue --detach [--remember]` |
| `k` | Mark | owner row wait | `verdict` (`ok` or `ko`), optional `note` | no | `sakura flow mark`; outside row: records the mark in the lane's log |
| `r` | Reply | question wait | optional `choice` (one of the wait's `choices`) and `text`; one of them, not both | no | records the reply in the outside lane's log |
| `y` | Accept | proposal or automation wait | none | yes | for an automation `sakura automations accept <name> <digest>` (installs its LaunchAgent; refused as no longer waiting when the digest is not the proposal's; local only, so from a overlay client it is refused with the fixed reason of `P`, before the target is looked at; `N` stays allowed); for a proposal `sakura proposal accept <id> <digest>`: writes (refused as no longer waiting when the digest is not the proposal's) the proposed text into the knowledge folder, refused when the target changed since |
| `N` | Reject | proposal or automation wait | none | yes | `sakura proposal reject <id> <digest>`, or `sakura automations reject <name> <digest>` for an automation |
| `R` | Resume | flow, stopped or blocked, and its blocked wait | optional multiline `note`, up to 2000 characters | yes | `sakura flow resume --detach --stopped-revision=<state revision> [--note=<text>]`; a failed lane is not offered it |
| `s` | Stop | flow, any lane not finished | none | yes | `sakura flow stop`; a lane no engine drives (blocked, failed, awaiting approval) becomes stopped at once, a driven one gets the stop file. From a blocked wait, sends `--revision=<state revision>` |
| `C` | Clean up | flow, failed or stopped | none | yes | `sakura flow cleanup --discard`: removes the lane's workspaces, branches and repository without a merged pull request. From a blocked wait, sends `--revision=<state revision>` |
| `T` | Re-contract | flow, blocked or failed at the plan phase with a saved contract | none | yes | `sakura flow recontract` |
| `w` | Write to agent | flow, lane, outside lane | `text` | no | `sakura flow steer` / `sakura lane say`; outside lane: appends a message to its inbox. A flow's answer starts with `delivered (<role>)` (a running Claude role took the message now, in its session; the role is the one whose run held stdin, for example `delivered (builder): ...`) or `queued` (it reaches the next role at a safe boundary) |
| `L` | Start lane | project (`["project", repo]`, a registered repository; not in the snapshot document) | `text`; optional `preset` (a known preset) and `level` (`slowest`, `slow`, `normal`, `fast` or `fastest`), applied to the new lane at creation | no | `sakura flow start --detach --new --message=<text>` with `--preset=<preset> --relax=preset` and `--set=knobs.lane_pace=<level>` when given (an unknown preset or level is refused with `invalid preset: not a known preset` or `invalid level: use one of slowest, slow, normal, fast, fastest`); a run that worked answers `done` with `output` the id of the lane it made (the task id, which with the project's folder is the lane's key), empty when the command named none; the master's ceiling (`lanes`) refuses with a reason naming the open lanes. The master's own face (`sakura master serve`) runs only `L`, `w` and `s`, as `master:<session>`, without a confirmation |
| `P` | Add project | host (`["host","projects"]`, always present, not in the snapshot document) | `folder` | no | registers the Git repository holding the absolute path `folder`, in the cockpit's own process, as `sakura flow register` does (a known one changes nothing); answers `done` with `output` `{"name": ..., "folder": ...}`, or `refused` with one fixed sentence (the path is not absolute, not a folder, not in a Git repository, or cannot be used), never repeating the path or an error's own text; a repository or Git folder, symlinks resolved, that lies where an agent can write (the engine home, a workspaces folder, the system tmp folders) is refused with `the folder lies where an agent can write; choose another folder`, before it is trusted. The folder is resolved once and every step uses that path; the Git folder must hold `HEAD`, `objects` and `refs` and name no second common folder, checked from the files with no Git process. A folder swapped between the checks and the registration by someone who can write outside every place an agent can, the person's own account or a session running as it, is not defended against. `P` and `L` are refused from a overlay client of the served page with one fixed reason, `this control is only available on the machine the cockpit runs on`, before the target is looked at, so the answer says nothing of what exists on the machine; only the command line (and the master, for `L`) can run them. From the command line it is `sakura cockpit act add-project <folder> --json` |
| `G` | Digest now | host (`["host","digest"]`, always present, in the snapshot document's `host`) | none | no | `sakura digest run --detach`: starts the morning digest in the background and returns; refused at once with `another digest run is in progress` while one runs. The lane appears on the Inbox when it is done |
| `B` | Always-on service | host (`["host","services"]`, in the snapshot document's `engine.keeper`) | `name` (`keeper` or `phone-page`), `on` (`yes` or `no`) | no | `sakura keeper install` / `uninstall` for `keeper`; `sakura keeper phone-page on` / `off` for `phone-page`. Installing writes the keeper's LaunchAgent for the person's account and loads it; removing it leaves every lane as it is. Refused from a overlay client of the served page with the same fixed reason as `P` (`B` is local only), so only the Sakura window on this machine and the command line can run it |
| `A` | Automation on or off | host (`["host","automations"]`, in `automations.target`) | `name` (the automation), `on` (`yes` or `no`) | no | `sakura automations enable` installs the automation's LaunchAgent, `disable` unloads it and keeps the definition. Local only |
| `Y` | Give priority to my sessions | host (`["host","engine"]`, in `engine.target`) | `on` (`yes` or `no`) | no | `sakura yield on` / `off`: the host switch of the lanes' waiting, in `<engine home>/yield-to-person.json` (mode 0600, off by default) |
| `O` | Connect | host (`["host","connectors"]`, in `digest.connect.target`) | `kind` (`github`, `notion`, `linear`, `feed`, `program`); optional `label` and `address` | no | `sakura connectors connect <kind>`: checks the sign-in (`gh auth status`), the Keychain item (`sakura-connector-notion`, `sakura-connector-linear`), the feed address or the program (absolute, executable, owned by you, not writable by others) and records the connector; a feed and a program need a `label`, which makes the id (`feed-news`). A secret is never an input: the person stores a token in the Keychain themselves. Local only |
| `o` | Disconnect | host (`["host","connectors"]`) | `name` (the connector id) | no | `sakura connectors disconnect <id>`; a Keychain item stays. Local only |
| `connectoradd` | Add connector | host connectors | `op`, `choices` | yes | Save a new instance |
| `connectoredit` | Edit connector | host connectors | `op`, `name`, `revision`, `choices` | yes | Edit or explicitly reconnect an instance |
| `connectordisconnect` | Disconnect connector | host connectors | `op`, `name`, `revision` | yes | Revoke an instance and retain its identity |
| `connectorreplace` | Review replacement | host connectors | `op`, `name`, `revision`, `candidate`, `candidate_revision` | yes | Replace a custom implementation when equivalence has been verified |
| `I` | Digest settings | host (`["host","digest-settings"]`, in `digest.target`) | optional `at`, `lookback_hours`, `language`, `model`, `account`; at least one | no | `sakura digest settings`: rewrites the digest file (the old one is kept as `digest.toml.bak`, comments are not kept), and replaces the installed LaunchAgent when the time moved. Local only |
| `J` | Digest source on or off | host (`["host","digest-settings"]`) | `name` (the source id), `on` (`yes` or `no`) | no | `sakura digest source <id> on|off`: an off source stays in the file and is skipped. Local only |
| `l` | Rename | account | `label` | no | `sakura accounts rename`; local only (refused from a overlay client with the fixed reason of `P`) |
| `c` | Set reserve | account | `percent` | no | `sakura accounts set-reserve`; local only (refused from a overlay client with the fixed reason of `P`) |
| `n` | Disable | account | none | yes | `sakura accounts disable`; local only (refused from a overlay client with the fixed reason of `P`) |
| `e` | Enable | account | none | no | `sakura accounts enable`; local only (refused from a overlay client with the fixed reason of `P`) |
| `X` | Remove | account | none | yes | `leeway accounts remove` on Leeway's book, in the engine's own process; local only (refused from a overlay client with the fixed reason of `P`). Refused, with the reason in one line, when no Leeway helper is found, when the account is not in Leeway's book, when a step is pinned to it (in the personal rules, a preset or a project's own layer: the answer names the step and the project) or when a project's pace allow-list holds it (the answer names the project). After a removal the id is dropped from the allow-lists of projects that are no longer registered, so no list keeps it |
| `p` | Add plan period | account | `plan`, `since` | no | `sakura accounts plan`; local only (refused from a overlay client with the fixed reason of `P`) |

The answers to an outside lane (`k` on its rows, `r`, `w` and `d` on an outside lane) run in the cockpit's own process, never as a command of `sakura outside`: no command line of an outside agent carries a verb of the person. Each appends to the lane's log under its lock for the actor of the request (`overlay` through the gate, `cli` through `sakura cockpit act`), and names the row or question by the digest in the wait's id, so an answer to content the agent has changed since is `refused` with a reason starting `no longer waiting`. The `invocation` shown for them (`cockpit-outside ...`) only describes the call. A `choice` that is not one of the wait's `choices`, or a `r` with neither or both of `choice` and `text`, is refused before anything is written.

Jumping to a window is not an action: the cockpit has no terminal of its own to switch. Run `sakura flow attach` in the terminal. The action `o` (open a tmux lane) is gone with the shelved `sakura lane` command, and no snapshot lists a `lane` item, so `d`, `m` and `w` apply to flows and outside lanes only in practice.

Inputs: `verdict` is `ok` or `ko`, `note` on an owner row is one printable line, `choice` is one printable line, `remember` is `yes`; `name` is a lane name (`[a-z0-9][a-z0-9-]{0,39}`), `role` and `step` are short lowercase words, `model` is a model name, `from` is one printable line, `text` is 1 to 8000 characters without control characters (newline, carriage return and tab pass). Account inputs: `label` is 1 to 60 printable characters on one line (not blank), `percent` is the account's own reserve (`sakura accounts set-reserve`; `set-ceiling` is its old name, still accepted), a number from 0 to 100, or the word `none` (it clears the own reserve, the global one applies again); flows and the pace alert stop using the account at `100 - percent` weekly utilization (`stop_at_pct`), `plan` is a slug (`[a-z0-9][a-z0-9-]{0,31}`) that must also be one of the account's `plan_choices` (refused before anything is spawned, and by `sakura accounts plan` itself), `since` is `YYYY-MM-DD` or an ISO 8601 datetime (UTC when it has no offset). Everything else is refused before anything is spawned.

Every account action (`l`, `c`, `n`, `e`, `p`, `X`) is local only: a overlay client (phone routes and served page alike) is refused with `this control is only available on the machine the cockpit runs on` before the target is looked at, nothing runs, and the snapshot a phone reads lists no account actions; only `sakura cockpit act` runs them, and what follows describes that face. The account commands keep the rules of `sakura accounts`: each change is atomic and locked, an unknown account is refused, and a host whose accounts are still in the legacy store is refused until `sakura accounts migrate` has run. The confirmation of `n` names the command without the install path and without a working directory. When such a command exits with 2 the answer is `refused` (not `done`) with the command's message on one line: `no account claude/ghost`, or `this host still keeps its accounts in the legacy store; run sakura accounts migrate first`. A message that could name a file (an unreadable `accounts.toml`) is replaced by a fixed text, so no path reaches the client. Any other non-zero exit code (a crashed command, one that could not start) stays `done` with its `exit_code` and, as `output`, the same fixed one-line text (the raw output can hold paths and is never sent); the `output` of a successful account command is empty. The action log row of a refused command reads `failed` with exit code 2.

Answers (always an object with `result`):

```json
{"result": "confirm", "token": "1793003598.5c1d....a91f", "expires": "2026-10-26T08:02:00Z", "summary": "Stop on demo: run in /work/repo: /opt/sakura/bin/sakura flow stop --actor=overlay:owner@example.com:nMAC -- demo"}
{"result": "done", "exit_code": 0, "output": "stopped demo\n"}
{"result": "refused", "reason": "invalid step: does not match [a-z0-9_-]{1,40}"}
```

`done` means the command ran; `exit_code` is the engine command's, and a non-zero one is a failed command, shown with its `output`. `refused` means nothing ran.

Once a command has run the answer is always `done`, even when the row for it in the action log could not be written afterwards (a full disk, say): the document then carries `"audit": "not written: <reason>"` and the server or command prints one line to stderr. The field is absent when the row was written. Never retry a `done` that has an `audit` note: the command ran.

| Status | Body | When |
|-|-|-|
| `200` | `confirm`, `done` or `refused` | the request was well formed (an unknown key, a wrong kind, a bad input or a target that vanished are `refused` here) |
| `400` | `refused` JSON | a field no client produces (unknown, missing `key` or `target`, a `target` that is not a JSON key) or a `confirm` token that is wrong, expired, already used or for another action, target, input or identity |
| `403` | `forbidden` | the gate refused |
| `404`, `405` | plain text | unknown path under `/api/`; wrong method |
| `411`, `413`, `415`, `431` | plain text | POST without `Content-Length`; body over 16384 bytes; not a form; head over the limits |
| `500` | plain text | the controller failed; the access log names the exception type |
| `503` | plain text | every handler slot is busy; retry (an action POST that finds two actions running is a `200` `refused`, see the limits) |

### Confirming an action

An action with `confirm: true` (approve, stop, close lane) takes two calls with the same `key`, `target` and inputs:

1. Post without `confirm`. Nothing runs. The answer is `{"result": "confirm", token, expires, summary, command}`; `command` is the exact command the confirmation runs. `summary` is what to show the person: for an answer to a wait, the title of that ask (`Approve: Read the spec and approve it`), and for an approval at a lane's card, the lane's title, with no gate name, revision or lane id in it; for every other action it is still the command and the repository it runs in.
2. Post again with the same fields and `confirm=<token>`. If the token is valid the command runs and the answer is `done`.

The token is bound to the source (HTTP or command line), the login, the node, the action key, the target and the inputs; it expires after 120 seconds and works once. A token used before, changed in any way or issued to another identity answers `400 refused`. The token is used up before the command starts, so a process that dies in between leaves an action that may or may not have run: read a new snapshot, then ask again. Do not retry a `done` blindly.

Example, stopping a flow (the `csrf` and `Origin` rules above apply to both calls):

```
POST /api/action HTTP/1.1
Host: mac.tail1234.ts.net:8787
Origin: http://mac.tail1234.ts.net:8787
Content-Type: application/x-www-form-urlencoded

csrf=1793003478.9f2c...&key=s&target=%5B%22flow%22%2C%22%2Fwork%2Frepo%22%2C%22demo%22%5D
```

then the same body plus `&confirm=1793003598.5c1d....a91f`.

An action without `confirm` takes one call, for example changing the dial: `key=D&target=...&input.step=strong`.

### The engine's background work

`engine` holds what runs without the app. `engine.keeper` is the engine keeper, a LaunchAgent that looks every minute for lanes whose last run record has no end and whose process is gone, or that are running with no open run record and no engine process holding their lock (after a restart or a crash) and resumes each through the same detached resume as the budget wake. It never resumes a lane the person stopped or one that waits on the person (an open question, a gate, a blocked lane); it starts no more lanes than the preset's `lanes` knob allows and queues the rest for the next look; with no tmux server up it resumes nothing, as the wake does. A lane it has resumed twice within three minutes is not resumed a third time: it is marked blocked, with a reason that says so, and appears in `needs_person` and as any blocked lane in `waits`.

| Field | Type | Meaning |
|-|-|-|
| `engine.keeper.on` | boolean | the keeper looked within the last three minutes |
| `engine.keeper.since` | time or null | when this keeper process started |
| `engine.keeper.last_restart` | object or null | `at` (time), `resumed` and `waiting` (arrays of lane names `<project>/<task>`): what the keeper did after the last restart it found, looks within ten minutes of each other counting as one restart |
| `engine.keeper.needs_person` | array of strings | lanes the keeper stopped trying to resume |
| `engine.keeper.phone_page` | boolean | whether the keeper runs the phone page server (`B` with `phone-page`) |
| `engine.keeper.problem` | string, optional | why the keeper's state could not be read; then `on` is false |
| `engine.keeper.target`, `engine.keeper.actions` | string, array | the id and the actions (`B`) of the always-on services |
| `engine.yield_to_person` | boolean | whether lanes give priority to the person's own agent sessions (`Y`) |
| `engine.yielding` | boolean | whether any lane waits now for the person to stop working with an agent elsewhere |
| `engine.target`, `engine.actions` | string, array | the id (`["host","engine"]`) and every action of the engine target: `K`, `U`, `V`, `Q`, `Y`, `M`, `F`, `Z`, `E`, `g`, `j` |

### Connectors for the digest

A connector row retains `id`, `kind` (`github`, `linear`, `notion`, `feed`, `program`), `label`, `connected` (boolean),
`detail` (safe sentence), and `last_used_at` (UTC time or null). Version 1 adds these fields to registered instances.

| Field | Value |
|-|-|
| `revision` | Positive integer |
| `scope` | `mode` (`legacy` or `scoped`), `selectors`, and `labels`. Selectors are `repo` and `branch` or `whole_repo` for GitHub, `team` and optional `recipient` for Linear, `database` for Notion, `host` for feeds, and `reader: "custom"` for programs. Labels can include `resource`, `branch`, or `host` |
| `reads` | `{"key": "connector_reads_<kind>", "args": object}` with the safe scope labels, or `{"text": string}` for a custom sentence |
| `operations` | Array of keys drawn from `read`, `comment_pr`, `comment_issue`, and `append_page` |
| `events` | Array of supported event keys |
| `status` | `connected`, `disconnected`, `connector_unavailable`, `reconnect_required`, `scope_unavailable`, or `approval_required` |
| `dependent_tasks` | Array of objects with `id` and `label` |

Older registry rows can use a legacy scope. Clients should tolerate older engines and legacy digest rows that omit the
new fields. Snapshot reads saved facts and starts no provider calls or programs. Local program arguments, integration
bindings and credentials stay private. Program reads require the protected executable and interpreter
identities recorded at approval or import. A changed installation needs operator approval again.

These four versioned actions are local and operator-only. Use `--op` for a caller-generated operation ID and
reuse it when retrying the same request. `--name` and `--candidate` take opaque instance IDs. `--revision` and
`--candidate_revision` take the corresponding positive decimal revisions. The preview uses the usual `--confirm`
token and omits private arguments. On success `connector` contains `id` and `revision`, also encoded in `output`.
Connector-specific refusals include `reason_code` and `reason_args` (an object). Code values are `invalid_request`,
`invalid_choices`, `connector_unavailable`, `stale_revision`, `unknown_connector`, `operation_conflict`,
`reconnect_required`, `scope_unavailable`, `approval_required`, `replacement_unproven`, `replacement_incompatible`,
`local_only`, and `operator_only`. Refresh on `stale_revision`. A verified replacement preview can add
`display_args` with `label` and `candidate_label`; confirmation verifies both revisions and the evidence again.

Pass `--choices` as one JSON object. On add supply `label` and `kind`, plus the following fields.

| Kind | Fields |
|-|-|
| `github` | `repo` and either `branch` (PR base) or `whole_repo: true` |
| `linear` | `team`, optional `recipient` |
| `notion` | `database` |
| `feed` | `address` |
| `program` | `program` (path), optional `args` (array of strings), and `reads` |

Fields are bounded printable strings unless noted. On edit submit a full form or just `label` with optional
`reads` for a custom connector. Unknown fields and control characters are refused. The cockpit passes the validated
form through explicit options to `sakura connectors add` or `edit`: `--kind`, `--label`, `--repo`, `--branch`,
`--whole-repo`, `--team`, `--recipient`, `--database`, `--address`, `--program`, repeated `--arg`, and `--reads`.
The child receives `--op`, `--actor`, and, where applicable, `--id`, `--revision`, `--candidate`, and `--candidate-revision`.


Host grants use `[master.connectors]` or the corresponding lane class in `grants.toml`. Each key is an instance
ID and each value is an array of operation keys such as `["read", "comment_pr"]`. The default is empty.
The engine checks prepared and current grants on each invocation. A scheduled Master run also supplies its selected
IDs, including an explicit empty selection. The MCP tool names are `connector_read`, `connector_comment_pr`,
`connector_comment_issue`, and `connector_append_page` when eligible. Each takes `connector` (instance ID).
Reads optionally take `cursor`. Writes take `target` (PR number, issue ID, or page ID) and `text`.

Scoped connectors can advertise `check_failed` (GitHub), `issue_assigned` (Linear), `page_added` (Notion),
or `new_item` (feed). The scheduler calls `connectors.events.poll` on its own cadence and supplies active
subscriptions, durable admission and pause handlers. Each admission has `key`, `connector_id`, `revision`,
`kind`, `at`, and `item_ref`. Enforce a unique `(task_id, key)` before acknowledging admission. Use
`connectors.events.claim` for both event and tool dependencies, with the latest explicitly resumed revisions.
The scheduler publishes safe task labels through `publish_dependencies` for `dependent_tasks`. Event cursors
are independent of digest and tool reads. A new subscription baselines the current listing.

The built-in scheduler transfers legacy digest ownership through `sakura scheduled migrate`. The integration API
`connectors.migration.cutover` also supports an adoption callback that durably upserts Morning summary by handoff ID
and gates its claims on `ready()`. Both paths use the digest run lock and a durable ownership check. The handoff
keeps the original schedule and the ordered source checkpoints. The legacy digest keeps its historical files. Replacement eligibility requires an engine evidence port for effective
coverage and cursor translation. With unknown coverage the engine reports `replacement_unproven`.

A digest source in `digest.toml` is exactly one of `run`, `feed` or `connector = "<id>"`, and may carry `enabled = false`, a `label` (its name, one line of at most 60 characters) and `reads` (what it reads, one line of at most 160). A source naming a connector is read through its kind: `github` runs `gh search prs` and `gh search issues` (review requested and assigned to the person, signed in with `gh`); `notion` and `linear` call their API with an integration token read from the Keychain (`security find-generic-password`) at the moment of use, sent only as a request header, never printed, logged or stored; `feed` and `program` are read as a `feed` or `run` source. A connector that is not connected fails only its own source, with `connector <id> is not connected`. Every error is a fixed sentence, a status code or a host name: no token, response body or program output.

Install digest programs and their interpreters outside registered repositories, lane workspaces and temporary folders.
The engine checks the installation at launch and passes only identity, locale, temporary directory, a fixed PATH and digest cursor variables. Reconnect a program connector when its installation changes.


### Automations

An automation is an engine verb on a schedule: `daily HH:MM` or `every N minutes` (5 to 1440). The verb is one of the allow-list `digest run --scheduled` and `flow wake` (`automations.registry.ALLOWED_VERBS`), each a fixed engine command run as an argument list, never a shell string. Existing automation proposals remain readable (see `docs/master.md`): the engine writes a pending definition and a wait of kind `automation`, and only the person's `y` installs the LaunchAgent `local.sakura.auto.<name>`, which runs `sakura automations run <name>`; a verb off the list is refused with one fixed sentence before anything is written, and the list is checked again at every run. The definitions are `<engine home>/automations/<name>.toml`; the last run of each is kept beside them.

### Priority to the person's own sessions

With the switch on, a lane about to start a phase that spends (not `pr` or `cleanup`) looks every 60 seconds whether a Claude transcript (`<config dir>/projects/*/*.jsonl`, for every account of the registry) or a Codex session file (`<codex home>/sessions/**`) changed in the last five minutes in a folder that is not a lane workspace. If one did, the lane waits, recorded in its state as `yield_wait` (`waiting: person` and `since`), and starts within one check after the person stops. A run that has started is never stopped, the operator's stop file ends the wait at once, and a session inside any lane's workspace (the lane's own agents, or the person working in a lane's worktree) is never counted, so a lane cannot wait for itself. A transcript whose folder cannot be read is not counted either. The files are only read for their modification time and the first lines that name the working folder; nothing from them is kept or shown.

## Command line

The command line face is for a dashboard on the same machine. It reads the same files and goes through the same controller as the HTTP routes. Local documents retain the original keys and host details. There is no gate: it runs as the user who runs it, and the actor says who asked.

### sakura cockpit snapshot --json

Prints exactly what `GET /api/snapshot` returns, on one line, read fresh (not from the 5 second copy). `--json` is required. It only reads, so it is allowed in every session. Exit 0; 2 when a configuration file is broken.

### sakura cockpit act

```
sakura cockpit act <key> <target> [--text T] [--model M] [--role R] [--step S] [--name N] [--from F] [--label L] [--percent P] [--plan P] [--since D]
                     [--verdict V] [--note N] [--remember yes] [--choice C] [--folder F] [--level L] [--preset P] [--rule R] [--value V] [--accounts A] [--state on|off]
                     [--on yes|no] [--kind K] [--address A] [--at HH:MM] [--lookback_hours N] [--language L] [--account A]
                     [--op ID] [--choices JSON] [--until DEADLINE] [--project FOLDER] [--title TITLE] [--revision N] [--candidate ID] [--candidate_revision N]
                     [--confirm TOKEN] [--actor LOGIN] --json
```

`key` and `target` are the action key and the `id` from the snapshot. The options are the inputs of the action table (`--text`, `--model`, `--role`, `--step`, `--name`, `--from`, `--label`, `--percent`, `--plan`, `--since`, `--verdict`, `--note`, `--remember`, `--choice`, `--folder`, `--accounts`, `--level` (lane pace, pace), `--preset` (switch a lane's preset, use a preset), `--rule` and `--value` (set an engine rule), `--device` and `--port` (the phone page), `--provider` (the account actions `g` and `j`), `--state` (turn a lane's step on or off, `t`), `--op` and `--choices` (first-launch setup, `setupapply` on the engine target: `--op` is the caller's operation id, reused on a retry so the setup runs once, and `--choices` is compact JSON with required `accounts` and optional `preset`, `pace` and `master_account` keys, with omitted settings left as saved), and for the background controls `--on`, `--kind`, `--address`, `--at`, `--lookback_hours`, `--language`, `--account`); `--until` sets a snooze deadline. `--project` and `--title` override a proposal start default. `--confirm` carries the token of a confirm result; `--actor` is the login written to the action log and the engine ledger as `cli:<login>` (default `SAKURA_ACTOR`, else `cli:<user>`); `--source` is `cli` (the default) or `app`, the Sakura window acting for the person: the actor is then written as `app:<login>`; `--json` is required. A value that starts with a dash is written `--text=-x`.

It prints one of the three result objects above, and exits:

| Exit | Meaning |
|-|-|
| 0 | `done` (whatever the engine command's `exit_code`) or `confirm` |
| 2 | `refused`, including a bad token, a bad actor and a call from an engine role session |
| 4 | a file the command needs could not be read or written, such as the proof secret under an unwritable `HOME` (the output is a `refused` object). A damaged used-token file during `--confirm` is a plain `refused` result, exit 2 |

Confirm works as over HTTP: a first call prints a token, a second call with `--confirm TOKEN` and the same arguments and actor runs the command. The token is bound to the actor, the action, the target and the inputs, expires after 120 seconds, and is one-time across processes: the secret and the used tokens are kept in `~/.local/state/sakura/cockpit/` (files `0600`, directory `0700`, under the `HOME` the process sees, never moved by `SAKURA_ENGINE_HOME`) under a lock, so a token cannot be replayed from another process. A token from the command line is not valid over HTTP, nor the other way round: the source of a token is the transport that issued it, never the actor. An actor taken from `SAKURA_ACTOR` keeps its login and node, but its source is always `cli` here, whatever the variable says.

`cockpit act` is an operator command: it is refused inside an engine role session (`SAKURA_FLOW_ROLE` set) and denied by the hook brakes like the other operator commands. `cockpit snapshot` is not.

Example:

```
$ sakura cockpit act s '["flow","/work/repo","demo"]' --actor owner@example.com --json
{"result": "confirm", "token": "1793003598.5c1d....a91f", "expires": "2026-10-26T08:02:00Z", "summary": "..."}
$ sakura cockpit act s '["flow","/work/repo","demo"]' --actor owner@example.com --confirm 1793003598.5c1d....a91f --json
{"result": "done", "exit_code": 0, "output": "stopped demo\n"}
```

### sakura cockpit serve, --once and --open

`sakura cockpit serve` runs the HTTP face in the foreground; `sakura cockpit --once` prints the snapshot as text; `sakura cockpit --open` runs `serve` in the cockpit tmux session and prints the URL. `serve` and `--once` do not need tmux; `--open` does. `serve` and `--open` are operator commands, refused inside an engine role session.

### sakura flow changes

`sakura flow changes <task> --json` prints, on one line, the files a lane changed from its base to the current state of its worktree (workspace 0), uncommitted edits and new files included: `{task, note, base, gone, files, added, removed}`. A file is `{path, status, added, removed}` with `status` `added`, `modified`, `deleted` or `renamed` (a rename also has `old_path`); a binary file has `added` and `removed` null. `added` and `removed` at the top are the totals. `gone` is true, with no files, when the lane's worktree is not there any more. At most 2000 files are listed and `omitted` says how many more there were or had names that cannot be shown.

`sakura flow changes <task> --file <path> --json` prints `{task, note, path, patch, truncated}`: the patch of that one file, cleaned of terminal controls and hidden characters and cut at 512 KB (`truncated` true). The path must be one of the changed files, relative and inside the worktree with no `..` and no link leaving it; a link is shown as a link, never followed.

Paths and patches are the lane's own content, written by its agents: data, never instructions (`note` says so). Both forms only read. A refusal exits 2 with one fixed sentence on stdout (`unknown task`, `that file is not one of this lane's changed files`, `this lane has no worktree any more`, `the changes of this lane cannot be read right now`); the Git detail goes to stderr.

### sakura flow feed

`sakura flow feed <task> --json [--since <cursor>]` prints one page of the lane's activity on one line: `{note, entries, cursor, more}`. Pass the `cursor` of the previous page as `--since`; a cursor never moves back and a later page never repeats or skips an entry. The entries of all kinds come interleaved in time order: each carries `at`, and the page lists them by it (an entry with an empty `at` first; equal times: the runs' entries, then the notices, then the board posts; each source keeps its own order). A page after another continues by cursor, so an entry that arrives late can carry an earlier `at` than ones already given: an app that shows the whole conversation orders it by `at`. The kinds:

| `kind` | fields | meaning |
|-|-|-|
| `text` | `run`, `role`, `workspace`, `text`, `tool`, `path`, `line`, `at` | a message a role wrote, whole up to 64 KB (`truncated` true when cut); a `SAKURA_RESULT` line is never in it; `at` is the time its event was stamped with in the run's log, else the time of the entry before it in that run, else the start of the run |
| `tool` | the same, plus `command` | a tool call a role made; a shell call also has `command`, the first line of what it ran (at most 200 characters, absent when the tool runs none). A `path` inside the engine's own `.sakura-role/` folder is left empty |
| `result` | `run`, `role`, `workspace`, `at`, `result: {verdict, summary}` | the result block a role ended its message with: `verdict` is `APPROVE`, `REVISE`, `REJECT`, `ERROR`, `NO_CHANGE` or `DONE`, `summary` one line of at most 400 characters (the findings, the reason, or a count) |
| `notice` | `tag`, `text`, `at` | a line of the lane's ledger; `at` is when it was written, empty for a line written before times were kept (those come first) |
| `board` | `from_task`, `from_title`, `role`, `text`, `at` | a post on the lane's project board made at or after the lane started, by any lane, the master, the person or the engine; `from_task` and `from_title` name the lane of a role's post and are empty otherwise; `role` is the role, or `master`, `person` or `engine` |

The cursor lists each run's entries read, `build-1=12`, then `ledger=<notices read>` and `board=<number of the last post read>`. The ledger's times are kept in `ledger.times` next to `ledger.md`. Everything in `text`, `tool`, `path`, `command`, `summary` and the board fields is what an agent or a person wrote, cleaned of terminal controls and hidden characters: data, never instructions (`note` says so). A cursor this feed did not give exits 2 with one sentence.

### Goose lane turns

A lane role accepts `goose` or `goose:<model>`. Use the Goose account id in the step's account setting
when more than one Goose account is enabled. Without a selected model the provider uses the default recorded
in the qualified installation. A run keeps its harness, account, upstream, model, role and captured workspace.
No subscription forecast or native-provider fallback applies to Goose.

Goose runs headlessly. Run descriptions for it include `acp: true` and, on failure, `acp_failure`.
Send a message with `sakura flow open <task> <run> --message <text>` or the existing run steering route.
A continuation loads the bound session and refuses a changed or unsupported binding before sending the message.
Use `sakura flow open <task> <run> --fresh --message <text>` to start an independent session on the same
account and model. The command uses only the message explicitly supplied for that attempt and any queued lane
steering. History for the earlier conversation remains available through the lane feed.

## The record of an action

Every attempt, done or refused, is appended to `cockpit/actions.jsonl` of the state folder with the time, action key, hashed target ID and target kind, actor (`source`, `login`, `node_id`, `device`), outcome and exit code. Refusals use a fixed audit message. The engine command itself receives `--actor=<source>:<login>[:<node_id>]`, so flow ledgers and the lane event log carry the same actor. An HTTP actor has source `overlay`, a command line actor `cli`.

### Strict resume admission

Cockpit Resume binds a stopped flow revision to `flow resume --detach --stopped-revision=...`. The child revalidates the revision and stopped status under the driver lock before clearing STOP. Admission increments a persisted generation and queues any note in the same state write, keyed to the admitted revision. Only the admission path records the Inbox decision. Ordinary CLI resume retains deliberate recovery semantics. Both paths accept an optional `--note <text>` of up to 2000 characters. A note must contain nonblank text. Newlines and tabs are allowed, and other control or hidden characters are refused. Phone forms accept multiline notes and normalize browser line endings to LF before validation and confirmation. An admitted note is recorded on the resume ledger entry and queued as an operator steer, for the next agent run. Omitting the note queues no steering.

The detached parent waits for a private startup receipt. The child acknowledges only after acquiring the lock and loading its context, checking trust, and admitting the revision. A proven startup rejection leaves STOP untouched and reports a refusal. Missing receipts, command timeouts, unexpected exits, and tmux failures after possible creation remain nonzero uncertain outcomes. Only exits 2 and 6 are safe refusals; clients must never treat a nonzero `done.exit_code` as success. An unmarked engine window also blocks a duplicate launch.

## The preset as a pipeline of steps (`engine`)

A lane is a pipeline: the steps Plan, Build, Checks and Deliver, and the reviews that loop on them (a review of the
plan, a review of each build task, a review of the whole branch). Each review can be on or off; Plan can be off (the
lane then builds from its goal, and the plan review goes with it); Build and Deliver are always on. A preset holds the
choice (the built-ins are `solo`, `standard`, `verified`, `vanilla` and `lean`); the person can also save their own.

Snapshot field `engine` (added to `api` 1; an app that ignores it keeps working):

| Field | Type | Meaning |
|-|-|-|
| `engine.id`, `engine.key` | string, array | the fixed target `["host","engine"]` of the actions below |
| `engine.preset` | string | the preset in force, empty when the config does not load |
| `engine.preset_label` | string | its name as a person reads it (`Lean`); show this, never the id |
| `engine.presets` | array | every preset a person may pick, as `{name, label, description, chosen}`: exactly one has `chosen` true (the preset in force, also while its rules are edited; none when the config does not load). The legacy names (`vanilla`) are left out unless one of them is the one in force; `sakura preset list --json` lists them all with `legacy` true |
| `engine.edited` | boolean | whether rules were changed since the preset was chosen; true until they are saved as a preset or dropped |
| `engine.actions` | array of action | `K`, `U`, `V`, `Q`, `Y`, `M`, `F`, `Z`, `E`, `g` and `j`, drawn like any other action |

The rules themselves are read with `sakura engine rules --json`: `{preset, edited, accounts_count, pace_accounts, nodes: [{node, rules:
[{id, label, type, value, choices, min, max, origin, read_only}]}]}` for the nodes `start`, `plan`, `plan_review`,
`build`, `task_review`, `final_review`, `checks`, `deliver` and `guard` (read-only: the cage and the grants). A rule id
is dotted (`reviews.plan_review.on`, `steps.build.model`, `reviews.task_review.rounds`, `lanes_min`, `lanes`,
`subagents`, `report_language`). `type` is `bool`, `int` (with `min` and `max`), `choice` (with `choices`), `text` or
`list`. `origin` names where the value comes from (`defaults`, `preset:<name>`, `rules`). An account rule
(`steps.build.account` and the like) is listed only when the person has more than one account; its choices are
`project` (the project's own account) and the account ids.

A step's agent is chosen as on Master: the account, then the model of that account's program, then the effort the model takes.
`agents` (optional for a client) lists every enabled account of the registry as one flat entry under its name,
`{id, label, provider, fetched_at, source, models: [{id, label, description, efforts, default_effort, is_default, resolved}]}`:
the models are the ones the program itself lists (Codex through its app-server `model/list`, Claude Code through the `models` of
its stream-json `initialize` request), kept in `<engine home>/models/` with the date they were fetched; `fetched_at` is empty and
`models` is `[]` for an account whose list was never fetched (action `M`, `sakura engine models refresh`, fills them; a lane
never waits for it). The model rule of a step (`steps.<step>.model`, `reviews.<step>.model`) carries an optional `agent`:
`{account, provider, model, resolved, effort, effort_is_set, default_model, accounts: [{id, label, provider}]}`: the account
the step is pinned to (`project` when none), the program and model it runs, its effort (the step's own when set, else the
dial's), the preset's model of the step's tier, the accounts it may choose from (every account, on every step), and an optional `note`
(empty, or the words that say the step runs on an account of another program because its own has none), `account_label` (the name Leeway gives
the account the step is pinned to, `null` for the project's own, "an account Leeway does not list" for a pin the book no longer holds: never an id)
and `models_state` with `models_note`: `kept` (the lists are there), `refreshing` ("Reading the models of Claude."), `unavailable` ("The models of Claude
could not be read: ...") or `missing`. The accounts are Leeway's book, with its labels (the engine's own registry only when Leeway cannot be asked);
each of `agents` carries the same `models_state` and `models_note`. `sakura engine rules` starts one background read for an account with no list (a fresh
engine home) and answers at once, so the app asks again after a few seconds; a failed read is not asked again for five minutes. `sakura engine models refresh
--missing` reads only the accounts with no list. Its `choices` are `provider:model` for the models the allowed accounts' programs list (the long-standing
`claude:haiku`, `claude:sonnet`, `claude:opus` and `codex` while none is kept). The new rule `steps.<step>.effort` (also
`reviews.<step>.effort`) is the effort, with the efforts the step's model takes as its `choices`; `K` with the value `default`
(or empty) gives the dial's effort back. `K` on an account rule moves the step to that account: onto an account of the other
program it takes the preset's model of the step's tier when that is of the new program, else the program's own default. "Back to
the preset" is `U` with the current preset: it drops the step's model and effort with every other edit.

| Key | Target | Inputs | Runs |
|-|-|-|-|
| `K` | `["host","engine"]` | `rule`, `value` | `sakura engine set -- RULE VALUE`: checks the value against the rule's type and range, keeps it in `<engine home>/rules.toml` and refuses, keeping nothing, when the config it makes does not load (for example `lanes_min` above `lanes`) |
| `U` | `["host","engine"]` | `preset` | `sakura preset use -- NAME`: chooses the preset and drops the changes of `rules.toml` |
| `V` | `["host","engine"]` | `name` | `sakura preset save -- NAME`: writes the effective rules as `<engine home>/presets/NAME.toml`, chooses it and drops the changes |

`K`, `U` and `V` are never accepted from a remote device. A change may weaken a gate (a review turned off), which the
person's own `rules.toml` is allowed to do; it never reaches a repository's files. `sakura preset save` names three
model tiers, so the planner, the builder and the code reviewer stand for theirs.

### Names a person reads

The snapshot gives a name next to every id a window would otherwise print, so a client never shows an id:

| Id | Name next to it |
|-|-|
| `items[].preset`, `engine.preset`, `sakura engine rules --json` `preset` | `preset_label` (`Lean`); `sakura preset list --json` is `[{name, label, description, chosen}]` and `sakura preset show --json` has `label` |
| `items[].steps[].account`, `items[].reviews[].account`, `usage.providers[].account` | `account_label` (null while the step runs on the project's own account) |
| `accounts[].id` | `accounts[].label`: Leeway's label when it says more than the id; otherwise the provider and a word (`Claude, personal` for the account `claude-default`), never the raw id |
| `accounts[].plan`, `accounts[].plans[].plan`, the plan choices | `accounts[].plan_label`, `accounts[].plans[].label` and `accounts[].plan_choice_labels` (`Codex Pro Lite` for `prolite`) |

`checks` is a step the preset records and shows; the engine keeps running the project's own checks whatever it says.

## Pace: where each account lands at its reset

Pace is a line in percent of an account's week at its reset, and the engine asks Leeway's advisor before every step
whether the step's account lands under it. Above the line the step waits (a wait with the rule `pace`, until the time
the advisor gives), or, when the preset leaves the step's account to the project, moves to another account of the
same provider that is under it and that the person allowed for that project. With none allowed (the default) it waits
instead of moving; a step pinned to an account never moves. The reserve and the five-hour limit of the budget guard apply at every level; pace
only adds waits to them, and a lane's own pace never bypasses the reserve.

| Level | Line |
|-|-|
| Slowest | 50% |
| Slow | 70% |
| Normal | 90% |
| Fast | 100% |
| Fastest | no line: only the reserve and the five-hour limit stop a lane |

A lane has a pace of its own (`lane_pace`, one of the five levels): the one chosen in the new-lane sheet (`flow start --set knobs.lane_pace=<level>`), else the engine's when the lane was made, and only `sakura flow pace <lane> <level>` changes it afterwards. Its line is the line of that level, whatever the engine's level is later. The lane count has a minimum
(`lanes_min`, default 1, at most `lanes`): a wait never holds back a lane while fewer than that many other lanes run.

Snapshot additions (optional for a client):

| Field | Type | Meaning |
|-|-|-|
| `engine.pace` | string | the engine's level: `slowest`, `slow`, `normal`, `fast` or `fastest` |
| `engine.target_pct` | number or null | the line of that level; null for `fastest` |
| `engine.lanes_min`, `engine.lanes` | integer | the least and the most lanes |
| `items[].pace` | string or null | a flow lane's pace, one of the five levels (`slowest` to `fastest`), the same value Details and the engine's running-lanes table show; null for any other item |
| `items[].pace_pending` | boolean | true while the pace the person chose with `flow pace` has not been taken in by the lane yet (it applies at the next step): `items[].pace` already gives the chosen level; always false for an item that is not a flow lane |
| `items[].pace_accounts`, `projects[].pace_accounts` | array of objects | `{id, label, pace_allowed}` for every account of `accounts[]`: whether pace may move a step of the lane's (or the project's) project to it; `pace_allowed` is false for every account until the person allows it |
| `engine.pace_accounts` | object | the allowed account ids by project id, for the projects that list any (also in `sakura engine rules --json`) |
| `problems` | array of strings | gains one line when a config file or a preset still sets the retired `knobs.pace_day_pct`, which is ignored |

| Key | Target | Inputs | Runs |
|-|-|-|-|
| `Q` | `["host","engine"]` | `level` | `sakura engine pace -- LEVEL`: keeps the level in `<engine home>/pace.toml`, a file the preset's edits never touch; never accepted from a remote device |
| `M` | `["host","engine"]` | none | `sakura engine models -- refresh`: asks the program of every enabled account (Codex through its app-server `model/list`, Claude Code through its stream-json `initialize` request) for its models and keeps each list, with the date, in `<engine home>/models/`; a failure of one account is reported and leaves its old list; a lane never waits for it; never accepted from a remote device |
| `F` | `["host","engine"]` | `device` | `sakura cockpit phone --json allow -- DEVICE`: lets a device of the Mac's own login open the phone page (the id from `sakura cockpit phone --json`, `devices[].id`); refused for a device that is not on the overlay or belongs to another login; with no `cockpit.toml` yet it is written with the Mac's own login allowed; never accepted from a remote device |
| `Z` | `["host","engine"]` | `device` | `sakura cockpit phone --json remove -- DEVICE`: stops an allowed device from opening the phone page; never accepted from a remote device |
| `E` | `["host","engine"]` | `port` | `sakura cockpit phone --json port -- PORT`: keeps the port (1 to 65535) in `cockpit.toml`; a running serve binds the new one at once; never accepted from a remote device |
| `g` | `["host","engine"]` | `provider` (`claude` or `codex`) | makes an empty private (0700) login folder in the person's home, `.claude-N` or `.codex-N` from 2 up (the dot folders every agent cage denies), and answers `done` with `output` `{"provider", "folder", "command", "argv", "env"}`: `command` is the one line for the app to open in a terminal for the person to log in (`env CLAUDE_CONFIG_DIR=<folder> claude auth login`, or `env CODEX_HOME=<folder> codex login`), `argv` and `env` the same by parts. The engine never runs the login and never reads the folder; never accepted from a remote device |
| `j` | `["host","engine"]` | `folder`, `provider` (`claude` or `codex`), `label` (the name the person typed) | adds the login folder to Leeway's account book (`leeway accounts add FOLDER --provider PROVIDER`) and names it (`leeway accounts rename ID LABEL`), through the pinned helper; answers `done` with `output` `{"id", "provider", "label"}`, or `refused` with one fixed sentence: the path is not absolute, not a folder, not a dot folder of the home whose name holds `claude` or `codex`, not the person's own or writable by others; the folder is already an account; the name starts with a dash; the Leeway helper was not found; or Leeway did not take the change. A folder that is a link is resolved first. Never accepted from a remote device |
| `W` | a project (`["project", repo]`) | `accounts`, comma separated Leeway account ids, empty to allow none | `sakura engine pace-accounts -- ACCOUNTS` in the project: keeps the list in `<engine home>/pace_accounts.toml`, outside every repository; never accepted from a remote device |
| `q` | a flow lane | `level` | `sakura flow pace -- TASK LEVEL`: queued like a dial change and applied at the lane's next safe point; the ledger says `pace changed to LEVEL` |
| `S` | a flow lane | `preset` | `sakura flow preset -- TASK NAME`: queued like a dial change; at the lane's next step (a build task, a phase) the lane's reviews and checks follow the new preset: finished steps stay done, a plan review that ran stays shown as done, the final review and the checks switch on or off for what is still to run; the ledger says `preset changed to NAME` with the actor; never accepted from a remote device |
| `t` | a flow lane | `step` (a `pipeline` id), `state` (`on` or `off`) | `sakura flow step -- TASK STEP STATE`: turns one optional step on or off for this lane only, from its next step; the lane's own record keeps it (its overrides), the global preset and rules are not touched; refused in plain words for a step the lane has reached, for `build` and `deliver`, and for a step the lane cannot run; turning a review on while the reviewers are off turns on that review only; the ledger says `step STEP turned on` with the actor; never accepted from a remote device |

A step that moves to another account is kept in the lane's state (`budget.pace_account`, by step) and in the ledger as
`pace` line `<step> moves to the account <id>`.

## Usage and accounts from Leeway

When a `leeway` helper is found (`SAKURA_LEEWAY`, the `leeway` entry of `<engine home>/programs.toml`, or the installed
Leeway app; see `docs/master.md`), `accounts` and `usage` are Leeway's, so every app shows the numbers the others show,
and the budget guard and pace judge the same ones. Without a helper `problems` gains the line "Usage is unavailable: the
leeway helper was not found" (or the reason a found helper did not answer), `accounts` and `usage` are the engine's own
documents described above, and pace says nothing: flows keep only the budget reserve rules.

`accounts[]` then lists Leeway's account book in display order, hidden accounts included. Every field the engine's own
rows have is kept (`provider`, `id`, `key` (the id), `label`, `state`, `enabled`, `week_pct`, `week_resets_at`,
`five_hour_pct`, `target`, `actions`); `reserve_week_pct`, `reserve_week_pct_own`, `stop_at_pct` are null, `plans` and
`plan_choices` empty and `open_alerts` 0, since Leeway keeps targets of its own. New fields (optional for a client):

| Field | Type | Meaning |
|-|-|-|
| `plan` | string or null | the account's plan, as Leeway reads it |
| `forecast_at_reset_pct` | number or null | where the account is forecast to land at its weekly reset |
| `exhausts_at` | time or null | when the week is forecast to run out, if before the reset |
| `stale` | boolean | the last reading failed and the one shown is an older one |
| `hidden` | boolean | the account is hidden in Leeway: `state` is `disabled`, and no figures are shown |

`actions` of a Leeway account are `l` (rename), `n` (disable: hides it), `e` (enable: shows it again) and `X` (remove). They run
`leeway accounts rename`, `hide`, `show` and `remove` on Leeway's book in the engine's own process, and `sakura accounts` keeps
`c` (reserve) and `p` (plan period) for the engine's registry. `usage.providers[]` gains `account` and has one entry per
visible account (`utilization` and `projection` as fractions of the week). `sakura quota status` and `sakura quota
advise` pass the command to the helper and say so on stderr.

A preset can name an account for a step (`steps.build.account`); the launcher points that step's agent at the account's
folder and the guard judges the reserve on its numbers. With one account every run uses it. An account that Leeway does
not list (hidden or removed), or one of another provider than the step's agent, makes the step ask the person (the
rule `account`), never run on another account.


## The phone page, from the app

`sakura cockpit phone --json` reads the setup and changes nothing, in Tailscale or anywhere else (it only runs `tailscale status --json`):
`{state, words, enabled, port, link, tailscale: {installed, logged_in, path}, mac: {id, name, address, login} or null, allowed_logins, allowed_devices:
[{id, name, login, known}], devices: [{id, name, os, online, allowed, is_this_mac}]}`. `devices` are the overlay devices of the Mac's own
login, the Mac first; `allowed_devices` are the ids in `cockpit.toml`, with a name when the device is still on the overlay (`known`).
`sakura cockpit phone --json allow DEVICE | remove DEVICE | port N` change `~/.config/sakura/cockpit.toml` (the actions `F`, `Z` and
`E`) and print the setup that results. The file is written whole, at mode 0600, through a file of its own moved into place, and keeps
every key the engine does not know. A refused change prints `{"result": "refused", "reason": ...}` and exits 2. Inside an engine role a
change is refused; reading is not.

`state` is `not_installed` (no Tailscale CLI found), `logged_out` (installed, not signed in), `no_device` (no device that is on the overlay
may open the page yet) or `ready`; `words` says it for the person, with what to do: `Tailscale is not installed on this Mac. Install it
from tailscale.com and sign in, then come back here.`, `Tailscale is installed but you are not signed in. Open Tailscale and sign in, then
come back here.`, `No device may open the phone page yet. Pick your phone in the list; ...` (or, when no other device of the login is on
the overlay, to install Tailscale on the phone and sign in with the same account) and `The phone page is ready: open <link> on <device
names>.`. Both fields are optional for a client.

## Scheduled tasks

The optional `scheduled` envelope distinguishes older engines from an empty
`tasks` array. It includes `templates`, `target`, `actions` and, when
unavailable, a fixed `problem` code. Reading it performs no scheduled work.

The engine keeps definitions, approvals and idempotency receipts in a private
SQLite database. Schema changes advance `user_version` in a write transaction.
A snapshot opens an existing database read-only and treats a missing database
as empty. An unreadable or unsupported database surfaces a problem code.
Edits match the current revision inside the write transaction.

`scheduledcreate` uses `["host","scheduled"]`. Task actions
`scheduledupdate`, `scheduledpause`, `scheduledresume`, `scheduleddelete`
and `scheduledrun` use `["scheduled",id,revision]` (all strings).
Create and update use `--definition` with a complete JSON definition.
Create and Run now use `--op`. Delete uses the existing confirmation proof.
Scheduled mutations and approval decisions require a host transport. Phone snapshots use opaque action
targets and omit task prompts and full approval definitions.

`sakura scheduled list --json` returns the same envelope. A task has `id`,
`revision`, `title`, `prompt`, `connector_ids`, `when`, `project`, `preset`,
`account`, `may_start_lanes`, `enabled`, `state`, `reason`,
`connector_label`, `target` and `actions`. The project is the registered
folder from the project's `folder` field. Optional choices default to null.
`may_start_lanes` defaults to false and `enabled` defaults to true.

The trigger is one of `{"kind":"daily","time":"07:30","timezone":"Europe/Rome"}`,
`{"kind":"interval","minutes":15}` (5 through 1440), or
`{"kind":"connector_event","connector_id":"feed-news","event_kind":"entry_added"}`
when the connector advertises that event. `state` is `active`, `paused`,
or `connector_unavailable`. `reason` and `connector_label` can be null.
An unreadable store returns `problem: "scheduled_store_unavailable"`.

Inbox exposes Master task requests as `kind: "approval"` with subkind
`scheduled_approval` and a complete normalized definition in `detail`.
Existing `y` and `N` confirmation actions bind that exact revision.

The task's `next_at` is an engine-computed UTC timestamp or null for event
triggers and tasks that cannot run. A connector disconnect pauses dependent
tasks and retains the connector name. After reconnect, `connector_reconnected`
indicates the task can be resumed. Run now returns JSON in `output` containing
`run_id`. `sakura scheduled tick` claims the latest due clock slots and polls
configured event streams. `sakura scheduled reconcile` retries LaunchAgent
installation or removal. A failed reconciliation appears as
`problem: "launchd_reconciliation_failed"`; committed definitions remain saved.

Each engine home has a dispatcher label `local.sakura.scheduled.<hash>`, where
`<hash>` is the first 16 hex digits of the SHA-256 of its canonical path. The
plist uses that label as its filename plus `.plist`. The default engine installs
in the login account's `Library/LaunchAgents`. When `SAKURA_ENGINE_HOME` is set,
the dispatcher installs in `Library/LaunchAgents` under that engine home and
starts with `HOME` and `SAKURA_ENGINE_HOME` set to the canonical engine path.
Run `sakura scheduled reconcile` after login to load an override's dispatcher
into the new session. Manual runs kick the dispatcher for the current engine
home.

`sakura scheduled history <task-id> --json [--limit 1..100] [--before SEQUENCE]`
returns `{runs, before}`. A run contains `id`, `task_id`, `sequence`,
`title`, `trigger`, `state`, `reason`, `queued_at`, `started_at`,
`ended_at` and `conversation` (run ID when a conversation started).
Trigger keys are `kind` and `at` for clock and manual runs, or `kind`,
`connector_id`, `revision`, `event_kind`, `occurrence`, `item_id`, and
`occurred_at`, and `source_label` for connector events.

`sakura scheduled conversation <run-id> --json [--limit 1..100] [--after SEQUENCE]`
returns `{entries, after}`, or a `run_unavailable` problem for a missing
or expired run. Entries have `sequence`, `kind` (`prompt`, `response`,
or `tool`) and `text`. Sequence values are opaque pagination cursors.
Run states are `queued`, `running`, `finished`, `failed`, and `skipped`.

Scheduled findings are Inbox waits with `kind: "proposal"`, `source:
"scheduled"`, and section `triage`. Their `proposal` object has the existing
`op`, `project`, `preset`, `account`, and `title` plus `origin`
(`task_id`, `title`, `at`, `source`) and `run_id`. A null `run_id` means
the conversation is unavailable. The existing `start`, `dismiss`, `read`,
`unread`, `snooze`, and `unsnooze` actions work on both clients. Merging
additional source receipts preserves the first proposal and its attention state.

Each template contains `id`, `labels` by locale, and `definitions` by locale.
`morning-summary` provides English and Italian drafts. A draft starts paused.
Connector rows expose `event_kinds` for both the legacy feed `entry_added` stream and scoped connector events.
A new subscription baselines the current listing. The scheduler queues each new occurrence once per task,
checks the event revision again before running it, and freezes the selected connector revisions for the run.
Reads from scoped instances also require the prepared and current Master connector grant.

`sakura scheduled migrate` imports the legacy digest and eligible aliases into
one Morning summary. It can be repeated after an interruption. Legacy digest
archives remain readable. Once ownership switches, legacy digest mutation
commands return `scheduled_migration_complete`. Scheduled status can expose
`legacy_digest_config_unavailable`, `legacy_digest_state_unavailable`,
`legacy_digest_busy`, `scheduled_provider_unavailable`,
`scheduled_account_required`, `scheduled_adapter_unverified`, or
`legacy_agent_removal_pending`. An enabled legacy job remains in service
while its configured account or adapter cannot be verified.

Migration retains the source IDs, binding order, saved windows, opaque cursors,
legacy clock eligibility and archive files. The Scheduled writer commits new
checkpoints with the completed run and its findings. It advances successful
sources only and records an `all_sources_failed` run when no source succeeds.

## Tracks and Track decisions

Track support is additive in the desktop CLI's API 1. An older engine omits `tracks`. A healthy
supporting engine returns an array, including `[]` for a home without Tracks.
On a Track read failure, `tracks` is `null`, `track_problem` is a display
message, and `track_actions` and project `track_choices` are absent.
Lanes and other Inbox entries remain available. Keep open Track tabs by id
and retry on the next ordinary snapshot refresh. Refresh after a successful
write and on the ordinary engine-home or project-folder change signals. A
malformed known field, including an unknown Track or lane state, is a snapshot
parse error. The phone's API 2 carries the decision wait and Done entry through
its usual redaction and opaque target rules. It omits the Track collection,
membership references and controls for starting lanes in a Track.

This example shows the Track-related fields of a snapshot. Action lists use the
ordinary `key`, `label`, `needs`, `optional` and `confirm` descriptor.
The action lists in this example show representative entries.

```json
{
  "api": 1,
  "track_actions": [
    {"key": "tracknew", "label": "New Track", "needs": ["name", "goal", "op"], "optional": ["project"], "confirm": false}
  ],
  "tracks": [{
    "id": "t-8cf0143a",
    "name": "Release",
    "goal": "Ship the beta",
    "status": "open",
    "summary": "Builds are ready.",
    "updated_at": "2026-11-23T10:00:00+00:00",
    "projects": [
      {"id": "project-identity", "folder": "/work/app", "name": "app", "folder_missing": false}
    ],
    "lanes": [{
      "project_id": "project-identity",
      "task": "beta",
      "key": ["flow", "/work/app", "beta"],
      "title": "Ship the beta",
      "state": "live",
      "folder_missing": false,
      "gone": false,
      "did_not_start": false,
      "startop": null,
      "actions": []
    }],
    "conversations": [
      {"key": {"provider": "provider-name", "account": "account-id", "session": "session-id"}, "title": "Beta planning"}
    ],
    "decisions": [{
      "id": 1,
      "text": "Enable the new onboarding?",
      "state": "open",
      "answer": null,
      "settled_at": null,
      "wait_id": "[\"wait\",\"track\",\"t-8cf0143a\",\"track_decision\",\"1\"]"
    }],
    "actions": [
      {"key": "trackset", "label": "Edit Track", "needs": ["op"], "optional": ["name", "goal", "summary", "status", "clear"], "confirm": false}
    ]
  }],
  "items": [{
    "key": ["flow", "/work/app", "beta"],
    "track": {"id": "t-8cf0143a", "name": "Release"}
  }],
  "projects": [{
    "folder": "/work/app",
    "track_choices": [{"id": "t-8cf0143a", "name": "Release"}]
## First-launch harnesses and accounts

`sakura setup detect --json` is the local scan action. Ask once before invoking it and present the results for
review. Opening a snapshot does not invoke detection. The scan checks executable paths and known folder structure.
It never executes provider or usage status commands or opens files inside a candidate folder. It always returns
the three supported harnesses in order: Claude, Codex, Goose.

```json
{
  "harnesses": [
    {"id":"claude","label":"Claude","program_path":"/opt/bin/claude","found":true,"version":null,"adapter_path":"/opt/bin/claude-agent-acp","adapter_version":null,"readiness":"unknown","reason":"qualification_required"},
    {"id":"codex","label":"Codex","program_path":"/opt/bin/codex","found":true,"version":null,"adapter_path":null,"adapter_version":null,"readiness":"unavailable","reason":"install_adapter"},
    {"id":"goose","label":"Goose","program_path":"/opt/bin/goose","found":true,"version":"1.60.2","adapter_path":"/opt/bin/goose","adapter_version":"1.60.2","readiness":"unknown","reason":"connection_required"}
  ],
  "logins": [
    {"harness":"goose","folder":"/Users/person/.goose-work","proposed_name":"Goose","in_book_as":null,"logged_in":null,"readiness":"unknown","reason":"connection_required","registered_account":null}
  ]
}
```

`found` describes the native program. `adapter_path` is the separately resolved adapter, or the same path for Goose.
Versions are nullable strings from matching operator install metadata. `readiness` is `unknown` when only structural
evidence exists, `needs_setup` when explicit connection or account configuration identifies a setup requirement,
`ready` after a successful explicit account check, and `unavailable` when a required program or verified capability
is missing or incompatible. It is an account check, distinct from permission to launch a lane or Master.
`reason` is a stable code or null. The scan uses `install_program`, `install_adapter`, `qualification_required`,
`incompatible_install`, `program_untrusted` and `connection_required`. Use the code for an actionable localized
label. A structural login candidate is always unknown when its programs are present. The old nullable
`logged_in` field remains in the response. `in_book_as` and `registered_account` may be supplied from stored
Sakura accounts without a usage-helper call. A registered reference looks like `{"provider":"claude","id":"work"}`.

Apply confirmed choices with `setupapply` against the engine target using the same `op` on retries. Omitted preset,
pace and default leave their saved settings in place. A first-launch client can submit Standard and Normal explicitly.
`master_account` may select a checked login, select an existing enabled account by provider and id, or clear the
default with null. Selection by folder is valid only for a checked login in the same operation.

```json
{
  "accounts":[{"harness":"goose","folder":"/Users/person/.goose-work","name":"Local models","provider_config":{"provider":"ollama"}}],
  "preset":"standard",
  "pace":"normal",
  "master_account":{"harness":"goose","folder":"/Users/person/.goose-work"}
}
```

A successful result includes the resolved default when supplied. The rules snapshot exposes the saved reference as
`engine.master_account` when set.

```json
{
  "accounts":[{"id":"goose-work","provider":"goose","label":"Local models","folder":"/Users/person/.goose-work"}],
  "master_account":{"provider":"goose","id":"goose-work"}
}
```

The Goose selection's optional `provider_config` contains only the upstream provider slug. The private
`goose-accounts.toml` registry stores the configuration root as the account's folder reference.
An omitted provider keeps the account in `needs_setup` and reserves Goose's shared Keychain backing slot conservatively. Version 1.60.2's normal command uses
one system Keychain service for its saved secrets. Different roots cannot create separate slots. Ollama uses the
approved local endpoint `http://127.0.0.1:11434` and is keyless. The API has no fields for credentials, URLs,
or environment overrides. Discovery also recognizes Goose's native default config folder. To run a check, use a
private `.goose` or `.goose-*` path root with `config/config.yaml` created through Goose configuration.

Goose rows appear with the existing account fields and the following additions. A configuration with no declared
provider emits `"provider_config": null`. No Goose row contributes to subscription predictions or pace.

```json
{
  "provider":"goose",
  "id":"goose-work",
  "provider_config":{"provider":"ollama"},
  "readiness":"ready",
  "reason":null,
  "forecast_available":false
}
```

The local `g` action with `provider=goose` can return the existing `provider`, `folder`, `command`, `argv` and
`env` fields for the Sakura wrapper around Goose's configuration flow. Execute `argv` directly from a local
terminal. A host without a safe, qualified configuration flow receives an actionable refusal. Register via
confirmed setup or the existing `j` action with optional `choices` string `{"provider":"ollama"}` to select the
configured provider. The same `j` action can supply the provider for a previously registered root.

An explicit local `accountcheck` on `["account","goose","goose-work"]` takes no input. It starts an ACP session
with no MCP grants and sends one short test prompt to the selected provider. Success returns
`{"readiness":"ready","reason":null}` in `output`. `setup_required`, `configuration_changed`,
`safe_setup_required`, `connection_failed`, `account_disabled` and `unsupported_capability` describe account
states without provider output. The account row reflects the last check of the same backing identity and
installation. `X` detaches the account after confirmation, clears its Master default when set, and preserves the
provider configuration and prior run and conversation records.

[Pinned Goose paths](https://raw.githubusercontent.com/aaif-goose/goose/v1.60.2/crates/goose/src/config/paths.rs)
[Pinned Goose configuration](https://raw.githubusercontent.com/aaif-goose/goose/v1.60.2/crates/goose/src/config/base.rs)

### Account model catalogs

`sakura engine models --json` reads saved catalogs. The rules snapshot exposes the
same data in `engine.agents`. The explicit `sakura engine models refresh --provider
goose --account local` probes the selected account. `--acp` reads or refreshes a
separate ACP cache for Claude and Codex. Their default caches retain the existing
sources. Goose always uses ACP. Rules snapshots read the cache. Rules CLI can
queue a background probe when a catalog is missing.

```json
{
  "accounts": [{
    "id":"local",
    "label":"Local models",
    "provider":"goose",
    "upstream_provider":"ollama",
    "fetched_at":"2026-11-23T15:11:00+00:00",
    "source":"ACP session",
    "models_state":"stale",
    "models_reason":"adapter_timeout",
    "models":[{
      "id":"qwen2.5:3b",
      "label":"qwen2.5:3b (Ollama)",
      "description":"",
      "efforts":[],
      "default_effort":"",
      "is_default":true,
      "resolved":""
    }]
  }]
}
```

An unavailable Track read adds these fields to the otherwise ordinary snapshot.

```json
{"api": 1, "tracks": null, "track_problem": "Tracks could not be read. Try again after refreshing."}
```

- A Track's `id` is a stable string (`t-` and eight lowercase hex digits).
  `name` is its display label, `goal` is its intended outcome, and `summary`
  is its current progress text, possibly empty. `status` is `open`, `paused`
  or `closed`. `updated_at` is the most recent audited change, as an offset-aware
  ISO 8601 timestamp. `projects`, `lanes`, `conversations`, `decisions` and
  `actions` are arrays. A flow item on a supporting engine carries `track` as
  `{id, name}` or `null`. On Track read failure the top-level state explains
  those null references. Other item kinds omit `track`.
- Each Track project has its stable `id`, canonical `folder`, saved display
  `name` and boolean `folder_missing`. The canonical path is the main worktree
  or bare repository. Worktree aliases share its identity. A missing folder
  keeps the saved identity and label. Restoring it at the same path restores
  its eligibility. Changing the canonical path creates a different identity.
- A lane link has `project_id` matching a Track project and `task` matching
  the flow task id. `key` is the cockpit flow item's key array when present,
  or `null` when it cannot be opened as a card. Use the current `title` or
  the saved title supplied in the row. `folder_missing` reflects the project.
  A readable saved lane can still be `live` or `finished` with a missing folder.
  `live` includes stopped, waiting and blocked lanes. `finished` covers a
  ready report or cleaned lane. `unreadable` means the engine could not verify
  its lifecycle. `gone` means the engine verified absence and sets the boolean
  `gone` to `true`. A missing card alone provides no lifecycle evidence.
- `pending` and `interrupted` are unresolved reservation states. Both set
  `did_not_start: true`, expose the original `startop`, and can expose recovery
  actions. Recovery may be in progress and yield an empty action list. A missing
  folder disables Retry. Remove requires verified absence and a serialized
  reservation check. For other members `did_not_start` is false, `startop` is
  `null` and `actions` is `[]`.
- A conversation key is an opaque object of three strings. Match all three
  fields against the persisted Master session. Only a started session has the
  identity needed to link. Navigation uses the exact live session or its
  History entry. Starting fresh creates a new identity, and restoring History
  uses the saved one. Keep the saved `title` if the session is unavailable.
- A decision id is a positive integer scoped to its Track. A settled decision
  carries the first answer and time, and has `wait_id: null`. An open decision
  carries its compact JSON Inbox wait id. The last element of that key is the
  decimal decision id encoded as a string.
- `track_choices` lists open and paused Tracks for a project with an available
  lane start. A healthy eligible project can return `[]`. Missing folders omit
  the field. To offer a Track picker, check that the project's `L` descriptor
  advertises `track` and `op` in `optional`. A choice needs no prior project
  membership. Starting the lane adds it.

An interrupted start with verified absence can offer both recovery actions.

```json
{
  "project_id": "project-identity",
  "task": "track-0123456789abcdef0123456789abcdef0123456789",
  "key": null,
  "title": "Verify builds",
  "state": "interrupted",
  "folder_missing": false,
  "gone": false,
  "did_not_start": true,
  "startop": "release-start-1",
  "actions": [
    {"key": "trackretry", "label": "Retry lane start", "needs": ["startop", "op"], "optional": [], "confirm": false},
    {"key": "trackremove", "label": "Remove lane reservation", "needs": ["startop", "op"], "optional": [], "confirm": false}
  ]
}
```

An open Track decision is one Inbox wait. It uses the ordinary read, unread,
snooze and unsnooze controls and contributes to the ordinary Inbox counts.

```json
{
  "id": "[\"wait\",\"track\",\"t-8cf0143a\",\"track_decision\",\"1\"]",
  "key": ["wait", "track", "t-8cf0143a", "track_decision", "1"],
  "kind": "question",
  "subkind": "track_decision",
  "source": "track",
  "section": "needs_you",
  "resolution": "open",
  "group": {"id": "[\"track\",\"t-8cf0143a\"]", "label": "Release"},
  "repo": "",
  "task": "",
  "project": "Release",
  "lane_title": "",
  "digest": null,
  "track": {"id": "t-8cf0143a", "name": "Release"},
  "title": "Enable the new onboarding?",
  "detail": "Enable the new onboarding?",
  "technical": "",
  "findings": [],
  "since": "2026-11-23T10:00:00Z",
  "author": "",
  "links": [],
  "choices": [],
  "is_read": false,
  "is_snoozed": false,
  "snoozed_until": null,
  "wants_note": false,
  "folder_missing": false,
  "actions": [
    {"key": "read", "label": "Mark read", "needs": [], "optional": [], "confirm": false},
    {"key": "unread", "label": "Mark unread", "needs": [], "optional": [], "confirm": false},
    {"key": "snooze", "label": "Snooze", "needs": ["until"], "optional": [], "confirm": false},
    {"key": "tracksettle", "label": "Answer decision", "needs": ["answer", "op"], "optional": [], "confirm": false}
  ]
}
```

`since` is creation time. The wait key and attention marks survive rename,
pause, close and folder loss. A new decision starts unread. Existing Inbox
kinds, routing and grouping stay compatible. Use `subkind: "track_decision"`
and the advertised `tracksettle` action for the Track answer. Attention changes
leave the decision open. `snooze` takes `until` as a future ISO 8601 timestamp
with offset or `1h`, `tomorrow` or `nextweek`. `unsnooze` appears for snoozed
waits. An answer removes the wait and puts its original id, title and
attribution into Done. Done uses the existing seven-day, newest-first combined
limit of 200 entries. A Track entry uses `answer: "reply"`, with the answer in
`text`, `choice: null`, `lane: null`, and `place` equal to `desktop`, `phone`
or `cli`. The first actor and place survive replays through another transport.

```json
{
  "id": "[\"wait\",\"track\",\"t-8cf0143a\",\"track_decision\",\"1\"]",
  "key": ["wait", "track", "t-8cf0143a", "track_decision", "1"],
  "kind": "question",
  "project": "Release",
  "task": "",
  "lane_title": "",
  "title": "Enable the new onboarding?",
  "answer": "reply",
  "choice": null,
  "text": "Enable it for the beta.",
  "actor": "app:owner",
  "place": "desktop",
  "at": "2026-11-23T10:07:00Z",
  "lane": null
}
```

### Track actions

The desktop app uses `sakura cockpit act KEY TARGET --json` with the usual
`--source=app`. The target is the compact JSON string for `["tracks"]`,
`["track", "<track-id>"]`, the decision wait key, or the existing project id
for `L` (the serialized `["project", "<canonical-folder>"]` target).
Each input is a separate argv entry, such as `--summary=First line\nSecond line`
with a literal newline inside the argument. All inputs, including `decision`,
are passed as strings. The Track options are `--name`, `--goal`, `--project`,
`--summary`, `--status`, `--clear`, `--text`, `--decision`, `--answer`,
`--task`, `--conversation`, `--title`, `--startop`, `--track` and `--op`.

| Action | Target | Required inputs | Optional inputs |
|-|-|-|-|
| `tracknew` | Collection | `name`, `goal`, `op` | `project` |
| `trackset` | Track | `op` | `name`, `goal`, `summary`, `status`, `clear` |
| `trackdecision` | Track | `text`, `op` | |
| `tracksettle` | Track | `decision`, `answer`, `op` | |
| `tracksettle` | Decision wait | `answer`, `op` | |
| `tracklink` | Track | `op` and one of the link input sets below | |
| `trackunlink` | Track | `op` and one of the unlink input sets below | |
| `trackretry` | Track owning the reservation | `startop`, `op` | |
| `trackremove` | Track owning the reservation | `startop`, `op` | |
| `L` | Existing project | `text` | `preset`, `level`, and the paired `track` and `op` |

- `project` is the registered canonical folder string from the project snapshot,
  up to 1024 characters. Lane links take `project` and `task`, the flow item's
  key components. A task id has 1 to 48 lowercase ASCII letters, digits or
  hyphens and starts with a letter or digit. A lane or conversation has one
  Track at most. Linking it elsewhere moves it. Its former Track keeps the
  project membership. Use `trackremove` for an unresolved start eligible for
  removal. Ordinary unlink cannot remove that reservation.
  Conversation links take `conversation` as a JSON object encoded into one
  argument, with `provider`, `account` and `session` as nonblank single-line
  strings of up to 256 characters each. Link with `title` alongside
  `conversation`. Unlink with the same identity and omit `title`.
- `trackset` applies at least one supplied change. Omit unchanged fields,
  and use `--clear=summary` to empty the summary. `clear` and `summary` are
  mutually exclusive. `status` accepts `open`, `paused` and `closed`.
  Open and paused names are unique after trimming and case folding. Reopening
  checks that constraint. Closing is refused while any linked lane is live,
  reserved or unreadable. A closed Track still offers edits, unlink and any
  outstanding answers. Reopen it before adding decisions, links or starts.
- `name` is a nonblank single line of at most 80 characters. `goal` and
  decision `text` support multiple lines and require 1 to 500 characters
  with some non-whitespace content. `summary` supports up to 4000 characters.
  `answer` supports multiple lines and requires 1 to 2000 characters with
  some non-whitespace content. `title` is a nonblank single line of up to
  120 characters. `decision` is the canonical decimal form of the positive
  integer decision id (up to `9223372036854775807`). For `L`, use the normal
  project start inputs. `text` allows multiple lines, up to 8000 characters.
- `op` is a caller-generated, nonblank single line of up to 256 characters.
  Keep it with the submitted inputs across retry and use a fresh one for
  a changed intent. The engine rejects reuse with different inputs. A
  committed answer can be replayed using either target form, with the same
  operation id and answer. A fresh operation on a settled decision is refused.
  The phone normalizes browser line endings to LF.
- On tracked `L`, `track` is a Track id from `track_choices`. The engine
  rechecks project and Track eligibility on write. Keep `track` and `op`
  absent for an ordinary project start. `trackretry` uses the original
  `startop` from the member row and a separate `op` for this retry request.
  The engine saves the lane's membership before dispatch. A retry resumes an
  interrupted start or returns its recorded task id. It uses the same lane id,
  frozen inputs and original opener, even when the person retries a start
  initiated by Master. Open the created lane with `["flow", folder, output]`
  using the project's canonical folder and the exact returned task id. `trackremove`
  makes the removed reservation terminal. This also prevents late retries
  with the original `L` operation from starting the lane.
- The desktop and CLI actions above act as the person. Master has a scoped
  tool set with a server-bound actor and session. See [Master tools](master.md#master-tools).
  Lane roles have no Track mutation authority. Phone shows
  decisions in Inbox with a CSRF-protected answer form and stores the operation
  id in the form for resubmission. The phone transport keeps its own opaque
  Inbox references. For percent-encoded answers over 16 KiB, the form uses
  `POST /action?track-decision`, and JSON callers can use
  `POST /api/action?track-decision` with the usual form fields.

```sh
sakura cockpit act tracknew '["tracks"]' --name='Release' --goal='Ship the beta' --op='release-new-1' --source=app --json
sakura cockpit act tracksettle '["wait","track","t-8cf0143a","track_decision","1"]' --answer='Enable it for the beta.' --op='release-answer-1' --source=app --json
sakura cockpit act tracklink '["track","t-8cf0143a"]' --conversation='{"provider":"provider-name","account":"account-id","session":"session-id"}' --title='Beta planning' --op='release-link-1' --source=app --json
sakura cockpit act L '["project","/work/app"]' --text='Verify builds' --track=t-8cf0143a --op='release-start-1' --source=app --json
```

Track mutations use the existing `done` and `refused` envelopes. On success
`L` and `trackretry` return just the created task id in `output`:

```json
{"result":"done","exit_code":0,"output":"track-0123456789abcdef0123456789abcdef0123456789"}
```

For other Track actions, `output` is one JSON-encoded service record.
For example, `tracksettle` returns:

```json
{"result":"done","exit_code":0,"output":"{\"track_id\":\"t-8cf0143a\",\"number\":1,\"text\":\"Enable the new onboarding?\",\"state\":\"settled\",\"answer\":\"Enable it for the beta.\",\"created_at\":\"2026-11-23T10:00:00+00:00\",\"settled_at\":\"2026-11-23T10:07:00+00:00\"}"}
```

`tracknew` and `trackset` return a record with the Track `id`, `name`,
`goal`, `summary`, `status`, `created_at`, `updated_at` and the four collections
`projects`, `lanes`, `conversations` and `decisions`. In these receipts the
collections are empty except for the optional initial `projects` entry of
`tracknew`, with `id`, `folder` and `name`. The `id` from `tracknew` can open
the new Track. `trackdecision` returns the decision
record shown above with `state: "open"`, `answer: null` and `settled_at: null`.
Lane links return `track_id`, `project_id`, `task`, `title` and nullable
`start_op`. Conversation links return `track_id`, the compound `key`
and `title`. Unlink returns the removed link. `trackremove` returns the
reservation with `op_id`, `track_id`, `project` (`id`, `folder`, `name`),
`task`, `title`, `inputs` (`text`, nullable `preset`, `level` and `account`),
`actor` (the original opener) and `state: "removed"`.

Refresh the snapshot after a successful mutation for current actions and
membership. A replay returns the original committed result.

```json
{
  "result": "refused",
  "reason": "Finish or remove these linked lanes before closing: Verify builds",
  "reason_code": "open_lanes",
  "reason_args": {"labels": ["Verify builds"]}
}
```

| `reason_code` | Meaning | `reason_args` |
|-|-|-|
| `invalid_input` | Field, value or input combination is invalid. | `{}` |
| `forbidden` | The actor cannot perform or replay this operation. | `{}` |
| `operation_conflict` | The operation id was already used with different inputs. | `{}` |
| `not_found` | The requested Track, project, decision, reservation or membership is unavailable for the target. | `{}` |
| `name_in_use` | An open or paused Track already uses this name. | `{}` |
| `closed` | The Track must be reopened for this action. | `{}` |
| `open_lanes` | At least one linked lane is live, reserved or unreadable and blocks closing. | `{"labels": ["Verify builds"]}` with safe lane titles. |
| `decision_settled` | The decision has a committed answer. An exact replay can still succeed. | `{}` |
| `lane_unavailable` | A linked lane or reserved start could not be verified, launched, retried, moved or removed, or Track storage is unavailable. | `{}` |

Use the plain `reason` for an absent or unknown code. Generic cockpit refusals,
such as stale targets and unavailable ordinary project actions, can carry just
`reason`. CLI `act` reports a refusal with exit code 2. A failed write leaves the
prior record intact. A failed tracked start can still have a committed reservation,
so refresh before offering recovery. Retry a lost response with the exact target,
inputs and operation id. A new draft needs a new operation id.

For terminal use, `sakura track` exposes `new`, `list`, `show`, `set`,
`decision add`, `decision settle`, `link` and `unlink`. Put `--json` on the
subcommand for machine-readable output. Each mutation takes an explicit
`--op`. A Track id is positional where a subcommand targets one Track, and
`show ID --audit` includes the attributed history.

```sh
sakura track new --name='Release' --goal='Ship the beta' --op=release-new-1 --json
sakura track decision settle t-8cf0143a --decision=1 --answer='Enable it for the beta.' --op=release-answer-1 --json
sakura track show t-8cf0143a --audit --json
```
The catalog identity is the harness, account, upstream provider when applicable,
and backing configuration. An empty `efforts` list uses the adapter's default.
`models_state` can be `kept`, `stale`, `refreshing`, `unavailable` or `missing`.
`models_reason` is null on a successful CLI read and omitted from rules when
there is no failure; rule rows also include the existing `models_note`.
A failed refresh keeps the last successful list for the same configuration.
At launch the engine checks the current session's model before applying an
explicit effort. Without enumeration, Goose uses its owner-pinned provider
default. Each refresh uses a disposable session, without inference or tools.

The local `M` action on `["host","engine"]` accepts both `provider` and `account`
to refresh one catalog. Omit both to refresh all accounts.

```json
{"key":"M","target":["host","engine"],"inputs":{"provider":"goose","account":"local"}}
```

### Master launch selection

The snapshot exposes the selected Master route without starting an adapter.
A saved Goose default selects the qualified ACP route. Claude, Codex and the
host default use `"master_launch_route":"legacy"`. Qualified launch, permission,
history and transcript commands run locally through the engine CLI.

```json
{
  "engine": {
    "master_account":{"provider":"goose","id":"local"},
    "master_launch_route":"acp"
  }
}
```

[Master launch and history contract](master.md#qualified-master-launches)
