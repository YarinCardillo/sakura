# Outside agents

An outside agent is an agent session the engine did not start: an interactive Claude Code, Codex, or any script on
this Mac. It opens lanes, adds checks for the person and asks questions. They appear on the Waiting page (phone and
app) beside the engine's own waits, and the person's answer reaches the agent that asked.

The engine keeps each outside lane in its own folder under `<engine home>/outside/`: a record written only by the
lane's agent and a numbered log that also holds the person's answers. The engine decides nothing from an outside
agent's words: what waits for the person, the progress and the unread count are computed from these records.

## The verbs

`sakura outside <verb> --agent NAME`, or the same names as tools of `sakura outside serve` (see MCP below). Every
verb prints one JSON document. Exit code 0 is done, 2 is refused (nothing was written), 4 means a store file could
not be read or written. The agent selects its host-issued grant with `--agent` or `SAKURA_OUTSIDE_AGENT`
and passes its token in `SAKURA_OUTSIDE_GRANT`. A value that starts with a dash is written `--title=-x`.

From the agent's project folder, the host operator runs `sakura outside grant NAME VERB [VERB ...]` and gives the
returned token to that agent's launcher. Issuing a new token for the name replaces its grant for future sessions.
`digest` is reserved for the host. A grant limits the agent to its own lanes and to its specified verbs. On the host,
`list` and `show` without an agent or grant remain available for inspection.

| Verb | Arguments | Answer `result` |
|-|-|-|
| `open LANE` | `--title`, optional `--about`, `--link LABEL=URL` (up to 4), `--phase draft or ready` | `opened`, `updated`, `unchanged` |
| `row LANE ROW` | `--title`, `--step` (1 to 20), optional `--expected`, optional `--note` | `opened`, `updated`, `unchanged` |
| `ask LANE QUESTION` | `--text`, optional `--choice` (up to 6) | `opened`, `updated`, `unchanged`, with `push` |
| `close LANE` | `--outcome done or dropped` | `closed`, `unchanged` |
| `inbox LANE` | optional `--after N`, `--wait SECONDS` | `inbox` |
| `list` | optional `--agent NAME`, `--closed` | `lanes` |
| `show LANE` | none | `lane` |

Repeating a call with the same content answers `unchanged` and writes nothing, so a retry after a timeout is safe.
A row whose content changes waits for the person again. A question whose content changed after the person replied is
refused: ask it again under a new id (the refusal suggests one).

A row is a check only the person makes: a title, the steps to run, an optional expected result and an optional note. The
person marks it Ok or Ko. A question may offer choices; the person picks one or answers in words.

## Limits

| What | Limit |
|-|-|
| lane, row, question ids and agent name | `[a-z0-9][a-z0-9-]{0,39}` |
| title (200), step (300), link label (40), choice (80) | one line, no control, bidi or zero-width character |
| about (2000), expected (1000), note (500), question (2000), reply and message (8000) | text; newline, carriage return and tab pass |
| link | `https://` only, at most 500 characters, no whitespace |
| per lane | 4 links, 50 rows, 20 questions, 6 distinct choices per question |
| open outside lanes | 50 across all agents |
| inbox page | 50 entries |
| wait | 0 to 300 seconds on the command line, 0 to 50 over MCP |

Refused, with nothing written: inside an engine role (`SAKURA_FLOW_ROLE` set); an engine home this process cannot
write (a sandboxed shell; use the MCP server, which your client starts outside the sandbox); a bad id or name; a
limit; a lane another agent name opened; a closed lane; the cap of open lanes (close one with `sakura outside close`);
a cursor past the log; a question changed after its reply.

## The same verbs over MCP

`sakura outside serve --agent NAME` is a stdio MCP server (protocol 2025-06-18) with the tools named by the grant, taking the same arguments and answering the same JSON document as text (a refusal or error is a tool result
with `isError` set). Your agent's client starts it outside the shell sandbox, so it can write the engine home where a
sandboxed `sakura outside` cannot. It refuses to start inside an engine role.

The server's `inbox` takes `lane` and `wait` (0 to 50, under Codex's default 60 second tool timeout) and no `after`:
the server remembers, per lane, the cursor of its last answer and sends it with the next call. A session therefore
sees each entry once, and entries it never acknowledged come again to the next session of the same agent name.

The engine writes no agent's settings; copy a snippet and replace the path with the `bin/sakura` of your install.
For Claude Code, in private user settings with the grant supplied by its launcher environment:

```json
{
  "mcpServers": {
    "sakura": {
      "command": "/path/to/sakura",
      "args": ["outside", "serve", "--agent", "claude"]
    }
  }
}
```

For Codex, in `~/.codex/config.toml`:

```toml
[mcp_servers.sakura]
command = "/path/to/sakura"
args = ["outside", "serve", "--agent", "codex"]
```

## The inbox

The person's marks, replies and messages are lines of the lane's log, numbered. `inbox` returns the ones numbered
above the lane's cursor, oldest first, at most 50, with `cursor`, the number of the last log line it covered.

- `--after N` acknowledges: the cursor becomes the larger of itself and `N`, then the read returns what is above it.
  A cursor past the end of the log is refused.
- Without `--after` nothing is acknowledged: the same entries come back until a read passes them.
- `--wait S` blocks until an entry arrives or the deadline passes, polling once a second. At the deadline it returns
  no entries. A read that is cut off (a client timeout, a killed process) acknowledges nothing, so its entries come
  again.

So a reader that passes the cursor it was given sees each entry once. There is no wake: an idle session sees an
answer when it next reads; an agent that needs an answer before it goes on blocks on `inbox`.

Delivered and acknowledged are kept apart. The lane records the highest entry any read returned (`delivered_up_to`)
beside the acknowledged cursor (`read_up_to`). The person sees "N not read yet" for the entries above the delivered
one, so an answer shows as read as soon as the agent was shown it. Redelivery still starts after the acknowledged
cursor: an agent that was shown an entry and never acknowledged it (a session that ended, a killed process) gets it
again, and nothing is lost.

A new question pages the person once through the engine's notifier (night deferral included), at most once per lane
per 10 minutes. Rows never page. A failed page is reported as `"push": "failed"` and never fails the `ask`.

## Lifecycle

`open` starts a lane in `draft` unless it names `ready`; both are the agent's word. A lane with an open row or
question waits for the person, which the engine computes. `close` is the agent's (`done` or `dropped`) or the
person's (`dropped`). A closed lane leaves Waiting at once, its inbox stays readable and no new row, question or
answer is taken. It is kept 90 days and listed by `sakura outside list --closed`; an id is never reused while its
record exists.

## Attribution

An outside agent is the name it gives plus where it runs: the main worktree of the Git repository holding its working
folder, else that folder. The engine records the author of every lane and log line as `outside:<name>` with that
folder, and shows "From claude in demo". A lane belongs to the name that opened it: another name cannot update it,
close it or acknowledge its inbox. Two sessions that give the same name are one agent to the engine, which lets a new
session continue a lane.

This is not a security boundary. Outside agents run as your macOS user: any of them can give any name, read and write
the engine home, and run your own commands (`sakura cockpit act`, `sakura flow mark`, `sakura flow approve`), which
the engine records as `cli:<user>` exactly like you at the Mac. What attribution protects against:

1. Mix-ups between agents that use the verbs: a retry, a second tool or a reused id cannot change, close or drain the
   inbox of a lane another name opened.
2. Misleading display: every lane, row and question says who wrote it and where, and the waits, progress and unread
   count are engine records, never the agent's words.
3. The outside path reaching the engine: it takes only outside ids and bounded text, so no outside call can approve,
   mark, steer or stop an engine lane, or run a command.
4. A Mac answer taken for a phone answer: an answer's author is `overlay:<login>:<node>` only when it came through the
   cockpit gate, which refuses action posts from the Mac itself.

The gap closes only with a dedicated macOS user for agents.

## What outside agents may never do

Mark, approve, accept, reply or steer as the person, or touch an engine lane.

- Not offered: `sakura outside` and the MCP server carry the seven agent verbs and nothing else. There is no
  `mark`, `reply` or `steer` verb and no `--actor` option, and `close` closes only the caller's own lane (another
  agent name is refused).
- Answered only through the cockpit: the person's marks, replies, messages and close lane are actions of the
  cockpit's action table (`k`, `r`, `w`, `d`), which run in the cockpit process for an actor of source `overlay` (the
  app and the phone page, through the gate) or `cli` (`sakura cockpit act`). The hook brakes deny `sakura cockpit
  act` and every writing `sakura outside` verb in engine-launched sessions.
- Not preventable for a process of the same user: it can still run `sakura cockpit act`, which records it as
  `cli:<user>`, exactly like you at the Mac. You may add deny rules for the commands that answer as you to your own
  agent settings. They are advisory, because an agent can still run a command another way. In Claude Code settings:

```json
{
  "permissions": {
    "deny": [
      "Bash(sakura cockpit act:*)",
      "Bash(sakura flow approve:*)",
      "Bash(sakura flow mark:*)"
    ]
  }
}
```

  For Codex, forbid the same prefixes in your rules file, for example
  `prefix_rule(pattern=["sakura", "cockpit", "act"], decision="forbidden")` for each command above. Check the syntax
  against your client's version; the engine writes no agent's settings.

## The person's side

Waiting shows the open rows and questions with a "From <agent> in <project>" line. Ok, Ko and a choice are one press.
A reply in words and a message to the agent open a page of their own. Each answer names the digest of the row or
question as the person saw it, so an answer to content that has changed since is refused as "no longer waiting".
