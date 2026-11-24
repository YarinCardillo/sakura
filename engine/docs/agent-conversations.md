# Agent conversations: reading one agent and writing to it

Every role run of a lane is an agent the person can read and write to. Three commands carry it. All print one JSON value
and exit 0 unless the task or the arguments are wrong; a message the engine cannot deliver is an outcome, not an error.

## `sakura flow sessions <task> --json`

One item per run, oldest first. The fields that existed before (`name`, `role`, `phase`, `provider`, `model`, `effort`,
`route`, `reason`, `host`, `state`, `started_at`, `ended_at`, `exit_code`, `session_id`, `cwd`, `resume`,
`not_resumable`) keep their meaning, with one addition to `state`: a run recorded as running whose process is gone is
`orphaned` (the engine settles the record itself when it next starts). New fields:

| Field | Value |
|-|-|
| `step` | the pipeline step the run serves: `plan`, `plan_review`, `build`, `task_review`, `final_review`, `checks` or empty (records from before it was kept get their role's own step). The review of the contract is `plan_review` although it runs under the code reviewer role |
| `account` | the label of the account the run was launched under, or null for the default account |
| `resumes` | the run whose conversation this one continues, or null |
| `doing` | `{"kind": "tool", "tool", "detail", "since"}` while a tool has started and not finished (`detail` is the first line of its command, else its path inside the workspace; `since` is an ISO time), `{"kind": "text", "text"}` with the first line of the last text block otherwise, null when nothing was logged |
| `listening` | `"live"` (a running Claude run takes the message at once), `"queue"` (a running run reads it when it ends), `"resume"` (an ended run is continued with it), `"no"` |
| `not_listening` | one sentence for the person when `listening` is `"no"`, else null |
| `agent` | the key shared by exactly the runs that are rounds of one piece of work: the same step, the same phase (spec, contract, plan, build, review) and the same plan task. A resume `<run>.<n>` has its root run's key. Opaque: compare it, do not parse it |
| `round` | 1-based position of the run among the runs of its `agent`, in start order (a resume counts as the next round) |
| `task_id` | the plan task the run serves (`T1`), empty when it serves none (or the plan is not written) |
| `task_title` | that task's title from the plan, empty when none |

`doing` is read from the end of the log, never the whole log.

## `sakura flow feed <task> --run <run> [--since CURSOR] --json`

```json
{"entries": [], "cursor": "opaque", "live": {"text": "partial text"}, "state": "running", "more": false}
```

The conversation of `<run>` is every run of its chain: a run resumed by a message is a new run (`build-1.2` resumes
`build-1`), and asking for any of them gives the same conversation. `state` is the state of the newest run. `live` is the
text the newest run is writing now (null when it is not writing one or has ended); it is recomputed from the current
message's deltas at every call. `more` is true when the page was cut and the cursor should be used again at once. `continued` and the `+n` ids of text parts are the only additions to the entry shape of the lane feed.

Entries are the lane feed's (`text`, `tool`, `result`) plus `you` (a message the person sent; the engine's own prompt is
not an entry) and `engine` (the engine's line about the agent, in plain words in `text` with no run name, actor or folder, and as the engine wrote it in `technical`, "" when the words are the line; for a ledger line that names a run of the chain; also the engine's replies and what another
agent wrote to this one). Every entry has `id`, `at`, `run`, `role`. A tool entry has `target`, what the tool works on: its file (by its path in the workspace, else by its name alone, never a folder of this Mac), its pattern or query, or the first six words of its command; "" when its input names none. It also has `started_at` and `ended_at`
(null while it runs); when its result is read the same entry comes again with the same `id` and its `ended_at`, and the
reader replaces the one it has. Times come from marks the runner keeps next to each log (`<run>.times`), to within a
second, because Claude and Codex stamp nothing.

A text longer than 64K characters is delivered as consecutive `text` entries of at most that many characters each: every
part but the last has `"continued": true` (the next entry continues it without a break), and the parts after the first have
the id of the first plus `+1`, `+2`... (so no id repeats). A page may end between parts; the cursor then holds how much of
the text was given. `live` is the whole text of the unfinished message, however long it is. A Codex tool is one entry, sent
when it starts and sent again with its `ended_at` when it completes (the same `id`); a tool first seen completed is one
entry with both times.

The cursor is opaque. It also remembers which file each log was (a log that was replaced, rewritten or cut short is read
again from its start, and the reader replaces entries by `id`). It holds a byte offset per log, so a poll reads only new bytes plus the deltas of the message being
written. Claude runs stream partial text (`--include-partial-messages`); a Codex run delivers text per completed message
and its tools as they start and complete.

## `sakura flow steer <task> --run <run> <text> --json`

```json
{"outcome": "delivered", "run": "build-1", "words": "The builder has it now."}
```

`outcome` is one of:

- `delivered`: written into the live channel of the running Claude run.
- `queued`: held for a running run that takes no live input, or one that did not take it in time; a worker continues the
  agent with it when the run ends.
- `resumed`: the agent had ended; `run` is the new run (`build-1.2`) that continues its conversation, started through the
  same guard as any launch, with the text as its first message.
- `refused`: `words` says why (the folder is gone, the conversation is no longer on this Mac, the guard says wait, the
  role now runs on another agent, no such run).

The message goes to the newest run of the agent's chain. One run of a chain starts at a time: a second message sent while
one starts is delivered to it or held for it. Without `--run`, `steer` behaves as before.

The actor of the message is the `--actor` flag (`--actor=app:<user>` from the app, as every other app action passes it), else the `SAKURA_ACTOR` variable, else `cli:<user>`. It is kept on a held message and ledgered (`steer to <run> queued|resumed|refused actor=...`; a message delivered live is ledgered by the inbox with its actor).

### What a resumed run's work does to its lane

Commits a resumed run (`<run>.<n>`) leaves in its task's workspace are part of the lane (`flow/resumed_work.py`):

- The lane does not merge or move to its next phase while a resumed run of it is live; it waits and says so in the ledger.
- When the resume ended with commits the lane does not hold, the task goes through the same code review and merge as a fix round and the lane goes back to build, so the final review, the checks and the delivery include them.
- When the lane had gone on before the resume committed (it pushed its branch, or was delivered), it says in plain words what is not in the branch: a delivered lane, or one blocked after its push (at the pull request step, say), is `blocked` with `Your message to the builder produced 1 commit after the lane pushed its branch: it is not in the pushed branch. Resume the lane to push it before it goes on to the pull request.` followed by why it was stuck (`N commits ... they are not` for more; `... after the lane was delivered ... Resume the lane to deliver again.` for a delivered one). Resuming it reviews, merges and pushes the commits first. A stopped or failed lane keeps its status and says it in its ask (`... after the lane failed: it is not in the pushed branch. The lane failed, so nothing else was done with it.`).

Only commits of resumed runs count; a commit nobody asked for in a task's workspace stays ignored. A resume of the final fix builder, which works in workspace 0, is covered too: its commits join the engine's head and go through the final review.
