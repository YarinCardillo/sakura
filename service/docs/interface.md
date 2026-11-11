# Machine interface of `box`

A desktop app (Sakura) shows and drives agentbox. It runs ONLY the `box`
binary, never git or docker itself. Everything it shows comes from the commands below.

## The two places, and what each guarantees

A lavoro (a task) lives in one of two places, chosen when it is created (`box new ... --where mac|box`, the
default comes from the project) and recorded for good in a file only the host writes. The two are different
things, and no path of `box` mixes them.

- `box`: a clone in a container. The agent is confined (its own network behind the proxy, no capabilities,
  limits), its workspace is untrusted, the host never runs git in it, everything read from it goes through
  sandbox containers and the verified store, and its work reaches the person's repository only with `box take`.
  Every guarantee this document makes about confinement and untrusted workspaces is a guarantee about lavori in a
  box, and only about them.
- `mac`: a git worktree of the person's repository, with the agent running natively on the Mac. **The agent of a
  lavoro on the Mac runs as the person, with the person's privileges.** It can do
  whatever the person can do on this Mac: read and write any file, use the person's credentials, start programs,
  reach the network. So nothing in a worktree on the Mac is treated as hostile, and nothing of the box machinery
  applies to it: no sandbox reads, no verified import, no proxy, no limits, no `take`. `box` gives such a lavoro
  order (one worktree and branch per lavoro, one check at a time, a place for its logs), not confinement.

What keeps the two apart:
- A lavoro on the Mac never reaches a container. Nothing of it is ever mounted, no drop directory exists for it,
  the runner starts no container for it. The functions that mount a workspace or a drop directory, start a job
  container or read through the verified store refuse a task on the Mac themselves, whatever verb led there, and
  every verb that belongs to a box says so in one `box:` line with exit code 80. (Otherwise a box could be started
  on a worktree, write git metadata there, and the host's git would follow it: a way out of the box.)
- A task in a box is never read with host git: the host git functions refuse a task whose record does not say
  `mac`.
- Host git for a lavoro on the Mac never looks for the repository in the worktree. It is given `--git-dir` and
  `--work-tree` explicitly, from trusted values: the folder of the lavoro and the git directory `box` recorded
  when it created the worktree (`worktrees/<name>` in the project's repository, checked like the repository
  itself, see "What a repository must be"). Before every call the `.git` file of the worktree must still hold
  exactly `gitdir: <that directory>`; anything else (another directory, another spelling, a link, a git directory
  of its own, a folder that became a link) refuses, and git is not run.

Which path each verb takes, asserted by `tests/test_place_paths.sh` against one task of each place:

| verb | on the Mac | in a box |
|-|-|-|
| `new`, `tasks`, `status`, `facts`, `log`, `diff`, `tree`, `file`, `search`, `graph` | native (no Docker) | containers |
| `acp`, `verify`, `cancel`, `stop`, `rm` and the runner's jobs | native (no Docker) | containers |
| `play` | native | not built yet: one line, exit 81 |
| `shell` | native: the person's login shell in the folder, not confined | a container |
| `take`, `run`, `agent`, `attach` | do not apply: one line, exit 80 | containers |
| `result-log`, `journal` | files of the data directory | the same |

Exit codes that say "not for this place": 80 the verb does not apply to the place of the task, 81 Play in a box
is not built yet. (82 is `box file` for a file over 2 MiB.)

## Lavori on the Mac

A lavoro on the Mac is a git worktree of the project's repository at
`$AGENTBOX_HOME/work/<task>`, on a new branch `<prefix><task>` (the project's `BRANCH_PREFIX`, `sakura/` when it
names none) started from the base. The agent runs natively, as the person. Everything below needs no Docker and
starts no container, and with only lavori on the Mac nothing asks Docker for anything (`box status` looks for
volumes left by tasks in a box only when a task lives in a box). Nothing runs at rest: no daemon, no watcher.
The efficiency rule says that a read of a lavoro on the Mac starts nothing beyond git itself. One exception is
accepted: `tree`, `file` and `search` start `python3` once per call, with the same script that reads a task in a
box (one implementation for both places, walking paths through directory descriptors); the other reads (`facts`,
`status`, `log`, `diff`, `graph`) run git and `jq` only. Each ends when it has answered.

Host requirements for lavori on the Mac: `git`, `jq`, `perl`, `python3`; `npm` and `node` for `box acp` (the
adapter is installed on first use).

### What changes in the verbs that existed
- `box new <task> <project> [--base <branch>] [--where mac|box]`: the place is the one named, else the project's
  (`WHERE`), else `mac`. On the Mac: the worktree and the branch are made (`git worktree add -b`), a branch of
  that name that already exists is never taken over, then the project's `SETUP` command runs once (below). No
  dependencies are installed by `box` and no drop directory is made.
- `box tasks --json`, `box status --json`, `box facts <task> --json`: every task carries `"where": "mac" | "box"`
  (a task created before places existed is `box`). `status` and `facts` also carry `"folder"`: the absolute path
  of the worktree for a lavoro on the Mac (what a client passes as the working directory of an ACP session),
  `null` for a task in a box. For a lavoro on the Mac `taken` and `agent` are always `null`, `otherContainers` is
  `0`, `branch` is its real branch (`sakura/<task>`), and the git fields are read with plain git every time (no
  cache; `--fresh` changes nothing). The other fields mean what they mean for a box.
- `box log <task> [--json]`, `box diff <task> [--commit <sha>]`: the same shapes, read with plain git. The patch is
  cut at 8 MiB as a whole (a last line says so); the per-file limit of a box does not apply; untracked files are
  the first 1000, plain files and links only.
- `box log <task> --setup`: the last 400 lines of the output of the setup command (control characters removed).
  Exit 3 when no setup command ran; exit 80 for a task in a box.
- `box verify`, `box cancel`, the runner: see "Checks" below. `box stop <task>` ends the open conversation (exit
  1 with one line when none is open). `box rm`: see "Removal".
- `box shell <task>`: the person's own login shell in the folder of the lavoro (the project's subfolder when it
  has one). `box` starts `$SHELL -l` when `$SHELL` is an absolute path of a program, else `/bin/zsh -l`, and the
  shell takes the place of the `box` process: its exit code is what the caller sees. **It is not confined**, as
  everything of the Mac place: it is the person's shell, with the person's privileges and environment. No lock
  is held while it is open (a terminal lasts as long as the person wants); `box` keeps a record of the shell
  (its process, the boot time, its start time). `box rm` refuses while a terminal is open in the lavoro (exit 1,
  one line); `box rm --force` tells each open shell to hang up (SIGHUP), waits 3 s, kills one that stays, and
  goes on. A record of a terminal that was closed is dropped without a signal.
- `box take`, `box run`, `box agent`, `box attach`: one `box:` line that says why it does not apply
  and what to do instead (take: "the branch is already in your repository"), exit code 80.

New native tasks use the default `sakura/` branch prefix. Existing tasks keep their actual
branch, including branches created by earlier versions; no branch is renamed.

### Commands of the project that run natively
`SETUP`, `ARCHIVE`, `RUN` and the checks of a lavoro on the Mac are run by `bash -c` in the folder of the
project inside the worktree (`work/<task>/<SUBDIR>`), as the person, with the person's environment plus:
`SAKURA_TASK` (the task), `SAKURA_WORKTREE` (the worktree), `SAKURA_REPO` (the person's repository), `SAKURA_BASE`
(the base branch). No previous product environment aliases are exported. The same four values are also exposed as `CONDUCTOR_WORKSPACE_NAME`, `CONDUCTOR_WORKSPACE_PATH`,
`CONDUCTOR_ROOT_PATH` and `CONDUCTOR_DEFAULT_BRANCH`, for scripts written for that tool. Where `box` finds its own programs: the first thing `box` does is rebuild `PATH` for itself from the entries
it can trust: absolute ones, resolved to the folder they really are, outside the data directory of `box` and
not writable by everybody. `.` and every relative entry are dropped, and so is a worktree that was put in
`PATH`. Whether a folder is in the data directory is decided by what the folder is (the device and inode of
the folder and of each folder above it, against those of the data directory and of the folders directly in
it), never by how its path is spelled: another letter case, a link or another mount name of the same folder
is left out the same. A program whose path `box` records (node, npm, git) is checked once more against the
data directory in the same way. Everything `box` runs by name (git, sed, jq, perl, stat and the rest) is looked up there, whether `box`
stands in a worktree or was started from one (from its terminal, say), so a file of a worktree that happens to
be called `sed` or `git` never runs in the place of that program. The supervisors of a command (`env`, `perl`,
`bash`) are started by absolute paths as well, and what a command needs to know is computed before `box`
changes into the worktree. What the person runs through `box` gets the `PATH` the person had: the commands of
a project, the agent of a conversation and the shell of `box shell` find their programs as the person's own
shell would. That is theirs to choose. Each runs in a process
group of its own, without standard input and without the locks of `box`; when it ends, is cancelled or passes
its deadline the whole group is told to end and then killed, so what it started in that group is not left
behind.

Where `box` may have to end such a group later (Play, the conversation, a native check) it keeps a record of the
group: its id, the boot time of the machine and the start time of its leader. Both times are read with programs named by their
absolute paths (`/usr/sbin/sysctl` and `/bin/ps` on macOS; `/proc` on Linux), so a short `PATH`, such as the one
the app starts `box` with, does not leave them empty. When a record cannot be made the command is not left
running without one: it is ended and collected at once and the start fails (exit code 125 of the supervisor).
Before any signal all three are
compared with the machine as it is; a record that does not match (left by a crash or a reboot, the number given
to another process since) is discarded and nothing is signalled.
That holds for each signal, not once per ending: `box` asks a group to end, waits, and compares the record
again immediately before the kill that follows, so a number that changed hands during the wait is not killed.

What "no process left behind" covers, and what it does not: `box` ends the process group of the command, for
setup, archive, checks, Play and the conversation alike. A command that leaves its process group on purpose (a
new session with `setsid`, a double fork into another group) is outside this guarantee: on the Mac the commands
are the person's own and run with the person's privileges, and `box` does not track where their descendants go.
What `box` does guarantee about such a child is that nothing waits for it: the reader of a log stops 2 s after
the command it serves has ended, whoever still holds the pipe, and the conversation relay stops 2 s after the
adapter has ended even when its output is still open. So `box new`, the runner, `box play stop` and `box acp`
end when their command ends.
- `SETUP`: once, at `box new`, with the install deadline (`BOX_INSTALL_TIMEOUT`, 1800 s). Output to
  `$AGENTBOX_HOME/setup/<task>.log` (at most 4 MiB). When it fails `box new` exits 1, says the exit code and
  `box log <task> --setup`, and the lavoro is kept.
- `ARCHIVE`: before `box rm` removes the worktree, same deadline, output to `setup/<task>.archive.log`.

### Removal
`box rm <task> [--force]` on a lavoro of the Mac refuses (exit 1, one line) while the worktree has uncommitted
files, or commits that are neither in a remote-tracking branch nor in the base branch
(`git rev-list HEAD --not --remotes <base>` is not empty), unless `--force`. Then the `ARCHIVE` command runs; when
it fails the lavoro is kept unless `--force`. Then its requests are cancelled, its Play is stopped and its ports
given back, the worktree is removed through the person's repository (a rename into a folder of `box`, a last look, the deletion, then
`git worktree prune`) and its records and logs are deleted. **The branch is never deleted.** `box rm` deletes a folder only
when it is the worktree `box` made for the lavoro (its `.git` names the recorded git directory, which belongs to
the project's repository), also with `--force`, and never deletes by a path it has not just examined: whatever else has come to stand at `work/<task>` (another folder, another repository, a link) is left
alone, with exit 1 and one line. The folder is looked at twice: before anything runs, and again after the
archive command has run and the conversation and Play have been ended, immediately before the removal. It is
then moved into a folder of `box` by its name (a rename moves a link, never what it leads to), examined there
once more (its device and inode, its `.git`), and only then deleted; the entry in the repository is dropped by
`git worktree prune`, which goes by git's own record of the worktree, never by resolving the path. A link
that leads nowhere is not "a folder that is gone": it is refused like anything else that is not the worktree. A worktree folder that is gone leaves only the records and the entry in the
repository, which `box rm` cleans up.

### Accounts
The accounts of the Mac are the person's existing Claude logins: config directories named in
`$AGENTBOX_HOME/config`, one `ACCOUNT_<name>=<absolute path>` per line. The file is written on the first run of
any command, with `ACCOUNT_claude=$HOME/.claude` when that directory exists (`BOX_FIRST_ACCOUNTS` names other
logins to look for on that first run, as `<name>=<directory in the home>` pairs: `"personal=.claude
work=.claude-work"`), and is never changed by `box` afterwards (add a line by hand for another login). It is read
as data. `box` never opens a file inside such a directory, never prints where it is (not in `box accounts
--json`, not in a message), and there is no sign-in step.

`box accounts --json` lists the accounts of the boxes first, then those of the Mac in the order of the file:
```json
[ { "name": "personal", "where": "box", "loggedIn": true, "authMethod": "claude.ai", "checkedAt": "..." },
  { "name": "personal", "where": "mac", "exists": true, "loggedIn": true, "authMethod": null, "checkedAt": "..." } ]
```
For an account of the Mac `exists` says whether its directory is there. Nothing more is known without opening
its files: `loggedIn` is a boolean that repeats `exists` (it is there because every element has it as a boolean)
and `authMethod` is always `null`. Without Docker the accounts of the boxes are left out
and one `box:` line on stderr says so; the exit code is still 0.

A project names its default account (`ACCOUNT`, set with `--account <name>`).

### `box acp <task> [<account>]` on the Mac
The same Claude ACP adapter as in a box (`@agentclientprotocol/claude-agent-acp`, the version the image pins),
run natively in the worktree for the account that is named, else the `ACCOUNT` of the lavoro's project values;
with neither, exit 1 and one line. The account's config directory reaches the CLI through `CLAUDE_CONFIG_DIR`,
with one exception: for the default directory (`$HOME/.claude`, also under another name or spelling) the variable
is left out. The CLI keeps the login of a directory under a name that depends on whether the variable is set, so
setting it to the default directory makes it look for a login that was never made, and the agent answers that
the account is not signed in. Any other directory (a login made with the variable set, such as `$HOME/.claude-work`)
gets the variable. It is installed with `npm`
on the first conversation into `$AGENTBOX_HOME/tools/claude-agent-acp-<version>/` (whole or not at all; a
failure is exit 1 with the path of `tools/install.log`) and started from there afterwards, never fetched again.
It uses the copy of the Claude Code CLI that the adapter's SDK brings, not the person's `claude`.
`node` and `npm` are looked up once, at that installation, and recorded as absolute paths
(`tools/claude-agent-acp-<version>/box-runtime`); npm runs in the new folder under `tools/`, and every later
conversation starts the adapter with the recorded `node`. Neither inherits the environment `box` was started
in: the installation gets `HOME`, `USER`, `LOGNAME`, `TMPDIR`, `LANG`, `LC_ALL` and a `PATH` that begins where
`node` is; the adapter gets `HOME`, `USER`, `LOGNAME`, `SHELL`, `PATH`, `TMPDIR`, `LANG`, `LC_ALL`, `LC_CTYPE`,
`TERM`, `SSH_AUTH_SOCK`, `PWD` (the worktree) and `CLAUDE_CONFIG_DIR`. `BOX_ADAPTER_ENV` names further variables
to pass to both (space separated). `NODE_OPTIONS`, `NODE_PATH` and `npm_config_*` never pass, whoever names them:
they would load code of the worktree before the adapter, or change where its packages come from.

The channel is the one of a box: `lib/acp_proxy.pl` stands between client and adapter with the same rules
(client capabilities other than `session` stripped, of the agent's requests only `session/request_permission`
passes and of its notifications only `session/update`, lines over 8 MiB end the session with exit 65, the end
and stall deadlines with exit 66), standard error goes to `$AGENTBOX_HOME/acp/<task>.log` with the same byte
limit, and the exit codes are the same. These rules bound what reaches the client; they do not confine the
agent, which runs as the person. One front end per lavoro: a second `box acp` while one is open is exit 1 with
"a conversation is open for task '<task>'". The lock of the front end is held by `box acp` and by the relay it
starts, which is the supervisor of the session: when `box acp` itself is killed outright (SIGKILL, no cleanup),
the relay sees it through a lifetime pipe whose other end only `box acp` held, ends the adapter and its process
group, and only then lets go of the lock, so no second conversation runs beside a first one that lost its calling component.
The reader of the log, which is started before the relay, holds neither that lock nor the lifecycle lock while
it waits for the relay to open the log pipe, and ends by itself when no writer has come after 30 seconds
(`BOX_LOG_RENDEZVOUS_SECONDS`); the same holds for the conversation of a lavoro in a box. The adapter and everything it starts live in one process group,
ended when standard input closes (within `BOX_ACP_END_SECONDS`), on a signal to `box`, and by `box stop <task>`
(the open `box acp` then ends with 143); what is left of the group is killed. The client passes the lavoro's
`folder` as the `cwd` of `session/new`.

### Play
A project names `RUN` (the command), `PORTS` (how many ports a lavoro gets, 1 to 100, default 10) and `RUN_MODE`
(`one`, the default, or `many`), set with `--run`, `--ports`, `--run-mode`.
For native tasks, each Play start reads these three current values from the host-private
project file, including for tasks created before a run command was configured. Other task
values, including checks, setup, archive and the worktree folder, keep their creation-time
snapshot. A task in a box retains its existing behavior.

`box play <task> start|stop|status [--json]`, `box play <task> log`. With `--json` each of the three prints the
status after acting:
```json
{ "task": "m1", "running": true, "pid": 4711, "ports": [56000, 56001, 56002], "since": "2026-11-10T16:00:00Z" }
```
`pid` is the id of the process group of the command (`null` when it does not run), `ports` the whole range
reserved for the lavoro (`[]` before its first start), `since` when it was started (`null` when it does not
run). Without `--json` the three print one line for people.
- `start` runs `RUN` detached from `box` (a session of its own), as described under "Commands of the project",
  without a deadline, with `SAKURA_PORT` set to the first port of the range, `CONDUCTOR_PORT` to the same value
  and `SAKURA_PORTS` to the size of the range. The range is the lowest one from 56000 up that no other lavoro
  holds. A reservation stays with the task between starts when its port count is unchanged.
  If the current count changed, the next start reserves a new non-overlapping range; `box rm` releases it
  (`box` reserves the numbers among lavori; it does not check that another program is not listening on them).
  With `RUN_MODE=one`, starting the Play of a lavoro first stops the Plays of the other lavori of the same
  project. A Play that already runs is left as it is. A command that ends within half a second is not a Play:
  exit 1, with `box play <task> log` named. A project without a current `RUN` refuses start with
  exit 1 and one plain `box:` line naming `box project set <project> --run '<command>'`,
  even if the task snapshot still contains an older command. An already running preview
  keeps its process and environment until stopped; changed settings apply to its next start.
- `stop` ends the whole process group (asked, 3 s later killed). Not running is not an error.
- `log`: the last 400 lines of the output (control characters removed); the log is
  `$AGENTBOX_HOME/play/<task>.log`, cut at 8 MiB (`BOX_PLAY_MAX_LOG_BYTES`) with a last line that says so, and
  started again at each start. Exit 3 before the first output.
- For a task in a box: `box: Play in a box is not built yet (task '<task>' is in a box)`, exit code 81.

### Checks
What a check on the Mac proves, and what it does not. A check on the Mac runs the project's command in the
lavoro's own folder with the person's environment. It proves that the command passed while HEAD was the recorded
commit and the tracked files matched it. It is not isolated from the agent, which runs as the person: ignored
files such as installed dependencies, git filters, submodules and the service's own files are within its reach.
A check that must hold against the agent is a check of a lavoro in a box. So every result says where it ran:
`"where": "mac"` or `"where": "box"`, in the result file (`results/<task>/<id>.json`) and in each entry of
`results` in `box status --json` and `box facts --json` (a result written before this field existed ran in a
box and is shown as `box`; `box tasks --json` shows nothing of results). The app is expected to label a result
by its place.

`box verify <task> <verb> [--no-wait]` queues a request for the commit the worktree is at, as for a box, and
refuses (exit 1) when the worktree has uncommitted files: `commit first: a check is for a commit`. Without
`--no-wait` it waits for the result and prints the last 40 lines of the log, a line `---`, the line
`verb=... sha=... exit=... seconds=... peak_memory_bytes=...`, and ends with the exit code of the check (75 when
no runner runs, 77 when the wait is over), like the waiting verify of a box.

The runner (`box runner`, one loop for both places, one request at a time across all lavori) runs the command of
the verb natively, as described under "Commands of the project", with the verb deadline (`BOX_VERB_TIMEOUT`,
2700 s). It runs only when the worktree is at the commit of the request and has no uncommitted files; otherwise
the result has `exit_code` 2 and its log says why (`commit first: a check is for a commit`, or that the worktree
moved on). The agent can edit files at any moment, so nothing is checked first and looked at again later:
the runner takes one reading of the worktree before the command (the head; the tree its tracked files make, built
in an index of its own that starts from the head, never from the index of the worktree, with every tracked file
read again; the files git neither knows nor ignores) and that reading itself must show the commit that was asked
for and nothing else. The same reading is taken after the command and must be equal. When it is not, the result
is published with `exit_code` 2 and the line `the worktree changed while the check ran` in its log, never as a
pass, whatever the command returned. A reading that cannot be taken never counts as equal: before the command the
check is refused (`the state of the worktree cannot be read`), after it the result does not count. A worktree
whose index has assume-unchanged or skip-worktree entries is refused, by `box verify` (exit 1, one line) and by
the runner: git is told not to look at those files, so a check cannot vouch for them. A check that leaves files
behind that git does not ignore never passes. The limit: an edit that is
made and undone while the check runs is not seen; ruling that out would take a checkout of its own per check. The result file and `results` in `status` and `facts` have the fields of a box result: `exitCode`
(the command's; 124 at the deadline with `timed_out` true in the file), `seconds` (the runner's clock),
`changesChecks`, `checksTouched`, `checksReason` and `testsTouched` (the rules of a box, applied to
`git diff --name-status <base> <commit>` of the worktree's repository), `tests` (read from the log). The peak
memory is not measured (`0`). `box cancel <task> <verb>` ends a queued or running check (the process group of a
running one is ended) with a cancelled result; a result is final. The runner starts and stops without Docker
(`box runner`, `--daemon`, `--stop`); while Docker does not answer, the requests of tasks in a box stay queued
and those of lavori on the Mac run. Stopping the runner ends the native check that runs (its whole process
group); the request stays in the queue and the next runner runs it again.

### Reading: `box tree`, `box file`, `box search`, `box graph`
The same four verbs for both places. For a lavoro on the Mac they run natively; for a task in a box `tree`,
`file` and `search` run in a workspace container (what they print about an untrusted workspace is the agent's
word: validated and shown, never acted on) and `graph` reads the verified store.

`box tree <task> [--path <dir>] [--all] --json`: one directory level.
```json
{ "path": "sub/src",
  "entries": [ { "name": "app.ts", "kind": "file", "changed": true, "added": 2, "removed": 1 },
               { "name": "lib", "kind": "dir", "changed": false, "added": null, "removed": null } ],
  "truncated": false }
```
`path` is `""` for the root. `kind` is `file`, `dir`, `link` (a symbolic link, not followed), `other`, or
`deleted` (a file of the base that is gone from this level). `changed` says whether the entry differs from the
commit the lavoro left its base at: for a folder, whether anything below it does. `added` and `removed` are
lines, for files only: against the base for a file git tracks (`null` for a binary one), all its lines for a
file git does not know yet (`null` above 2 MiB). Entries are sorted by name, at most 2000 (`truncated`). Files
and folders git ignores are left out unless `--all`; `.git` is never listed. Exit 1 with one line when the path
is not a directory of the lavoro, climbs out of it or leaves it through a link.

`box file <task> <path> [--at <sha>]`: the bytes of one file on standard output, exactly as they are (not
sanitized: text to display, never terminal output). Without `--at` the file of the working tree, a link is
followed only when it stays inside the lavoro; with `--at <40 hex>` the regular file that commit has at the
path. Nothing is printed unless all of it can be: exit 82 and `box: the file is larger than 2 MiB: box file does
not print it` above 2097152 bytes; exit 3 when there is no such file; exit 1 for a path that is absolute, climbs
out, names the git metadata, or is (or passes through) a link that leaves the lavoro, and for a link or a folder
at a commit. `tree` and `file` reach a path through directory descriptors, one component at a time from the root
of the lavoro, never following a link by name: a link is read and walked the same way (one with an absolute
target is refused even when it points inside, at most 16 links per path), what was opened is examined on its
descriptor (a FIFO or a device is never read and never makes a read wait), and every read stops at 2 MiB plus
one byte. A file or folder that is swapped for a link while a read is under way cannot lead it out of the lavoro. The
walk starts at the folder of the lavoro itself, which is opened by its name below its parent (the work directory
of `box`, or the root of the container) without following a link and must have the device and inode `box`
recorded for it when the lavoro was made (`WORKTREE_IDENTITY` in its record): a worktree that is renamed and
replaced by a link or by another folder between the check and the read is not walked.

`box search <task> <text> --json`: file names for quick open.
```json
{ "query": "spec", "matches": ["sub/existing.spec.ts", "sub/spec/a.ts"], "truncated": false }
```
The files git tracks and the ones it does not know yet (not the ignored ones) whose path contains the text,
letter case ignored; a match in the file's own name comes before a match in a folder's, shorter paths first; at
most 200 (`truncated`). The text is 1 to 200 characters.

`box graph <project> --json [--limit <n>]`, `box graph <task> --json [--limit <n>]`: the commits to draw. A
name that is a project is read as the project (all its lavori); otherwise it is a task (that lavoro alone).
```json
{ "project": "app", "base": "main", "limit": 100,
  "lanes": [ { "lavoro": null, "branch": "main", "where": null, "head": "<40>", "base": null, "readable": true },
             { "lavoro": "m1", "branch": "sakura/m1", "where": "mac", "head": "<40>", "base": "<40>", "readable": true } ],
  "commits": [ { "sha": "<40>", "parents": ["<40>"], "subject": "...", "time": "2026-11-10T16:00:00Z",
                 "lavoro": "m1", "branches": ["sakura/m1"] } ] }
```
The first lane is the base branch (of the project, or of the one lavoro): its last `limit` commits (default
100, at most 1000). Each other lane is a lavoro: its branch, place, head, the commit it started from (`base`),
and its commits after that one (at most 200). A lavoro that cannot be read is a lane with `readable` false,
`head` null and no commits. `commits` holds every commit once, newest first: `lavoro` is the lavoro it belongs
to, `null` for a commit of the base branch (also when a lavoro's commit was merged into it); `branches` are the
branches of the lanes whose head it is. A parent that is older than the commits listed is simply not in the
list.

### Publishing
There is no verb for it. The agent of a lavoro on the Mac has the person's git and `gh`: to publish, the app
sends the agent a request, in its conversation, to push the branch and open a draft pull request. `box` only
shows the outcome afterwards (the commits in `box log` and `box graph`; `box rm` accepts a lavoro whose commits
are in a remote-tracking branch).

## Differences from the contract

Where the contract could not be met as written, or a choice was needed. Everything else is as specified.

- Host dependency: `jq` must be installed (`brew install jq`). JSON from workspaces is produced by python3 in
  the sandbox container and re-validated and re-encoded by jq on the host (patterns for shas, branch names and
  times, length limits, control characters in strings replaced by a space). A report whose workspace output
  fails validation counts as unreadable.
- Speed: workspace facts need a sandbox container per task per call; on a large real repository
  `box status --json` takes about 10 seconds for one task, almost all of it `git status`. Poll accordingly and
  use `box facts <task> --json` for one task.
  The workspaces are read concurrently, at most four at a time, and listed in the same order as before (alphabetical
  by task name). With five tasks this took 5.5 s instead of 7 s.
  `box tasks --json` is the inventory without any container.
- `taken.at` is the time of the last `take` journal event of the task; when the branch exists in the source
  repository but the journal has no such event (taken before the journal existed), it is the commit time.
- `box verify <task> <verb>`, waiting or with `--no-wait`, queues the request directly from the host (the host
  reads the sha through the sandbox and writes the queue file); no drop file is involved. The host never
  writes into the drop directory of a task: an agent writes that directory, and only the agent, from its
  container, asks through it. The result id is
  `<time>-<sha>-<verb>` with the whole sha of the commit, 40 characters (the name of the result file in
  `results/<task>/`). A shortened sha is never an identity: two commits can be made to share their first
  twelve characters, so a request is found, and a result accepted by whoever waits for it, only by the whole
  sha, the task and the verb, which the result file also says inside. The queue file, the job
  container and the job volumes are named `<result id>.<task>`, so two tasks at the same commit asking for the
  same verb in the same second are two requests with two results. The same task, verb and sha queued twice
  stays one request. The queue file is published atomically (written under a temporary name in the queue
  directory, then linked into place; an existing request is never replaced).
- `agent.state` is `running` only when Docker says running; every other state (created, paused, exited) is
  reported as `exited`.
- Detach keys of `box attach`: `ctrl-\,ctrl-\` (press ctrl-backslash twice). Claude Code uses ctrl-p, ctrl-q
  and ctrl-] itself.
- `box agent` without `--detach` also uses the container name `agentbox-agent-<task>` and is not removed at
  exit (so its exit code can be journaled); the next start of that task removes it. The `agent-start` event of
  an attached session is written just before Docker starts it, not after it ended.
- `box diff` prints the patch bytes exactly as git produced them (untrusted data, not sanitized: the app
  must treat it as text to display, never as terminal output). `box result-log` removes control characters
  other than newline and tab. Untracked files are limited to the first 1000, regular files and symlinks only
  (a FIFO or device in the workspace is skipped).
- `box log` without `--json` prints `<sha12> <subject>` lines for people.
- Sparse workspaces: a profile with `SUBDIR` gets, by default, a sparse working tree (the SUBDIR cone plus the
  top-level files; profile key `SPARSE=0` keeps the full tree). Files outside the cone are not on disk but are
  in the history: `dirtyFiles` and `filesChanged` count only what is checked out, `box log`, `box diff` and
  `box take` are unaffected. Tasks created before this change keep their full tree.
- Journal: events of tasks created before the journal existed are absent. `verify-queued` and `verify-done`
  are written by the host process that queued or finished the request (the runner for `verify-done`).

Rules for every command here:
- Output is one JSON document on stdout (or raw text where stated). Nothing else on stdout.
- Failure: non-zero exit, one line on stderr that a person can read. No partial JSON.
- Every string that comes from a workspace (branch names, commit subjects, paths, patch text) is untrusted
  data. Emit it JSON-encoded, never evaluated, never used to build a command.
- Workspace facts are read inside the no-network sandbox container, one container per task per call. The
  host never runs git in a workspace.
- Times are ISO 8601 UTC with `Z`. Sizes in MB as integers. Shas are full 40 hex.

## Read commands

### `box status --json`
```json
{
  "runner": {
    "state": "running | stopped",
    "current": { "task": "m1", "verb": "test", "sha": "<40>", "startedAt": "..." },
    "queue": [ { "task": "m2", "verb": "build", "sha": "<40>", "queuedAt": "..." } ]
  },
  "tasks": [
    {
      "name": "m1",
      "project": "sample-app",
      "profile": "sample-app",
      "where": "box",
      "folder": null,
      "checks": ["build", "rules", "test"],
      "base": "main",
      "baseSha": "<40>",
      "readable": true,
      "branch": "box/m1",
      "head": "<40>",
      "headSubject": "Add the thing",
      "headTime": "...",
      "ahead": 3,
      "dirtyFiles": 2,
      "filesChanged": 6,
      "insertions": 120,
      "deletions": 14,
      "taken": { "sha": "<40>", "at": "..." },
      "agent": { "container": "agentbox-agent-m1", "account": "personal", "state": "running | exited",
                 "cpuPercent": 12.5, "memoryMb": 812, "startedAt": "...", "exitCode": null },
      "otherContainers": 0,
      "results": {
        "test": { "sha": "<40>", "where": "box", "exitCode": 0, "seconds": 412, "finishedAt": "...", "cancelled": false,
                  "changesChecks": false, "checksTouched": [], "checksReason": null,
                  "testsTouched": { "modified": 0, "deleted": 0, "files": [] },
                  "tests": { "ran": 7880, "skipped": 0, "base": 7880, "baseSkipped": 0, "source": "log" }, "isHead": true }
      }
    }
  ]
}
```
`project` is the project of the task (`profile` is its older name and holds the same). `checks` are the verbs
the project names, the ones `box verify` accepts for the task; a project without checks gives `[]`, and the app
must then ask the person at take, because nothing can be verified. `base` is the branch the task started from
(its own, when `box new --base` named one).

`changesChecks` says whether the commit that was checked changes the files that define the checks: the tree
difference between the task's base and the checked sha touches a path matching the profile's `PROTECTED`
globs (relative to the profile's `SUBDIR`; `*` stays inside a name, `**` crosses directories). `checksTouched`
lists those paths (repository-relative, sorted, at most 20). A commit with `changesChecks: true` is never plain
green: the checks it passed may be ones it rewrote. `checksReason` (shown next to `changesChecks` by `box status` and `box facts`) is `null` when the checks were computed and says
why otherwise. Every result carries `changesChecks` (a boolean), `checksTouched`, `checksReason`,
`testsTouched` and `tests`, on every terminal path (passed, failed, timed out, cancelled, quarantined): the one
function that writes a result fills in the conservative values for whatever its caller does not say, so no path
can leave a field out. It fails closed: when the profile names no `PROTECTED` paths or the sandbox could not
compare the commit, `changesChecks` is `true`, `checksTouched` is empty and `checksReason` says why ("could not be
computed" or "the profile names no protected paths"). A commit that changes any path outside the
project's `SUBDIR` counts as a change too, with `checksReason` "the commit changes files outside the folder of the
project" and the paths in `checksTouched`: the command of a check can call a file that lies outside the folder,
and what is not looked at is not known to be harmless. The project can list paths that are safe to change
(`SAFE_OUTSIDE`, `box project add|set --safe-outside '<globs>'`, patterns from the root of the repository,
`safeOutside` in `box projects --json`); nothing is listed by default. `PROTECTED` patterns are tried on the path
inside `SUBDIR` and on the path from the root, so they protect a file wherever it lies. What a result needed and does not know counts as a change
too, with the reason: which existing test files the commit touched ("the test files the commit touches could not be computed", or
"the project names no test files" when the project has no `TEST_FILES`: a missing pattern does not show that
there are no tests, only that nobody looked, so every result of such a project counts as a change). Cancelled and quarantined results ran nothing: `changesChecks` is `true`, the lists are empty,
`testsTouched` and `tests` are `null` and `checksReason` is "not computed: the request was cancelled" or
"not computed: the request was quarantined".
The comparison runs in the no-network sandbox with the workspace read-only (`image/runner/changes.py`), at the
time the runner records the result; the host never runs git in a workspace. It does not trust the workspace's
repository either. Git does not re-hash an object when it reads it, so objects are never read from the agent's
store directly, and git that reads the workspace is never given the verified store. The workspace's own
`.git/config` and `.git/info/attributes` can name a clean filter, a program that git runs during `status` and
`diff`, so a container that reads the workspace may be the agent's by the time it answers. There are three
kinds of sandbox container, none with both the workspace and a writable store: a *workspace* container (the
workspace read-only, nothing else: it says where HEAD is and what the working tree holds, and makes the pack of
the commits), an *import* container (the store writable and the trusted source, no workspace: its only input is
that pack, cut at 1 GiB, every object re-hashed) and a *verified* container (the store read-only and the trusted
source, no workspace: `changes.py`, the package files, `log`, `diff --commit`, the `take` bundle and the job's
checkout run here, on commits the host names). Every such call works in a per-task *verified repository*
(`$AGENTBOX_HOME/verified/<task>.store`, mounted at `/verified`, written only by import containers; a
`<task>.git` beside it is the store of an older `box`, which workspace containers could write, and is not used
any more): its only alternate is the objects of the profile's
source repository (mounted read-only at `/trusted`), which is where the base commit and its trees come from. The
agent's commits after the base are packed from the workspace and imported with `index-pack --strict`, which
computes the hash of every object itself and checks the links; an object whose content does not match its name
is missing afterwards and the import fails closed (exit 1). The workspace is never an alternate. Replace objects,
grafts, refs, config and the auxiliary files an object store carries (commit-graph, multi-pack-index, bitmaps)
are ignored in every git call of the sandbox, which also reads no system or global config and disables hooks,
fsmonitor and external diff. The base is the sha recorded host-side when the task was created. The
job's checkout reads only from the verified repository and the trusted one (the workspace is not mounted) and
fails unless it ends exactly on the requested sha. The package files for the dependencies, `facts` (head,
subject, time, ahead), `log`, `diff --commit`, `changes.py` and the `take` bundle all use this same import; the
`take` fetch into the source repository also runs with `fsckObjects`. Only the working-tree figures of `facts`
(`dirtyFiles`, `filesChanged`, `insertions`, `deletions`) and the working-tree `diff` still read the workspace's
own index and files; they describe uncommitted files and are for display, never for a decision. A commit that
fails the import is a failed result (exit code of the failed step) with `changesChecks` true and no job runs on
it. Results recorded before this
field existed show `null`.

`testsTouched` says which existing test files the commit changed: `modified` and `deleted` count the files
matching the profile's `TEST_FILES` globs (for `sample-app`: `**/*.spec.ts`) that exist at the task's base and
are modified, retyped or deleted by the checked sha (a rename counts as a delete plus a new file); `files` lists
them (repository-relative, at most 20). New test files do not count. It comes from the same sandbox comparison as
`changesChecks` and is `null` when it could not be computed or the project has no `TEST_FILES`; in both cases
`changesChecks` is `true`. A commit that deletes or edits the tests that judge it is visible here.


`tests` is set only for the `test` verb. `ran` is the number of tests that passed plus failed and `skipped` the
skipped plus todo ones, read from the last vitest summary line of the log (`Tests  2 failed | 10 passed |
2 skipped (14)`; colours ignored). The parts must add up to the total in brackets, every number has at most 9 digits and there is exactly one
summary line (the agent controls what its tests print); otherwise, or when no such line exists, `tests` is
`null`, never a guess. The counts are what the test command printed, nothing more, and the object says so:
`"source": "log"`. The test command runs code of the commit, so a commit can print any summary line it likes
without running a single test; one line that parses shows that it was printed, not that tests ran. `box`
therefore keeps the counts out of every decision: they are no part of `changesChecks` or `checksReason`
(a result with counts and one without say the same about the checks), and nothing in `box take`, `box rm` or
the runner looks at them. They are for display, and the app is expected to label them
"as the test command reported", never as a count `box` vouches for. `base` and `baseSkipped` are the same counts from the most recent
`test` result recorded for the task's base commit by any task of the same suite (not cancelled, with readable
counts), `null` when there is none. A result with counts records its suite in the result file (`suite`: the
profile, its `SUBDIR` and the command of the verb); two profiles can share a repository and a base commit and
run different tests, so only a result whose three values are equal is a base. Results recorded before `suite`
existed are never a base. A drop in the agent's directory is an empty file whose name is the verb: its content is never
read and a name that is not exactly a verb of the profile is moved to quarantine, so a request cannot carry
arguments and the command that runs is always the profile's.`current`, `taken` and `agent` are `null` when absent. `results` holds the last result per verb the profile
names; a verb never run is absent. `filesChanged`, `insertions`, `deletions` compare the merge base with the
working tree (committed plus uncommitted, tracked files). An unreadable workspace gives `"readable": false`
and the git fields `null`; the rest of the report is still produced. `taken` is the sha of `box/<task>` in the
profile's REPO (the person's trusted repository, so the host may read that ref there) and when it was taken.

### `box tasks --json`
```json
[ { "name": "m1", "profile": "sample-app", "where": "box", "base": "main", "taken": false } ]
```
`where` is the place of the task, `mac` or `box`. For a lavoro on the Mac `taken` is always `false` (its branch
lives in the repository from the start; there is nothing to take).
The cheap inventory, in the order of `box status`. Read from the host-private task records and from the
branch `box/<task>` in the profile's source repository (`taken` is true when it exists). No container is
started and Docker need not run; about 0.2 s with five tasks. Use it to list tasks, then `box facts <task>`
for details.

### `box facts <task> --json [--fresh]`
The same object as one element of `tasks`. For refreshing one task after its files changed.

Which commands may answer from the cache: only `box status --json` and `box facts <task> --json`, which are for
display. They cache the workspace part of the report in `$AGENTBOX_HOME/cache/facts/<task>.json`. `box take`,
`box rm`, `box verify` (the sha it queues), the runner and anything that checks a result against HEAD never use it;
they read in full every time. `--fresh` (on `box facts` and `box status --json`) bypasses the cache and refreshes it.

The host reads no path of a workspace for the cache, not a byte and not a change time: the agent writes those
paths. Whether a cached answer still holds is asked of one workspace container, which answers before it runs any
git (`image/runner/facts.py`, `workspace_key.py`): it compares the key of the cached read with the key now, and
looks for a file or directory of the working tree whose inode change time is after the moment the cached read
began (edits git has not staged, new and deleted files; change time, not modification time, which `touch -t` and
rewriting in place can set). The key holds the head read as bytes from `.git/HEAD` and the branch ref (loose file,
else packed-refs) and the change time and size of `.git/HEAD`, `.git/index`, `.git/packed-refs` and the ref. Even
inside the container the path is walked one component at a time, each opened relative to the descriptor of its
parent with `O_NOFOLLOW` (`O_DIRECTORY` for directories), checked with `fstat`, and at most 16 KB are read from the
descriptor that was checked; HEAD must be exactly `ref: refs/heads/box/<task>` and the value a 40 hex sha. Anything
else (a symbolic link, a FIFO, a device, an oversized file) means there is no key and every call reads in full. The
host adds what it knows without the workspace: the task's base sha from its record and the state and start time of
the task's agent container, so starting or stopping an agent invalidates the answer.

What it costs: a cached answer used to start no container; it now starts one small workspace container (no git,
a walk of the working tree's metadata), and a full read is that container plus one verified container. A cached
answer is served only if its `head` equals the head of its key, so `head` and `isHead` are never stale. Only
successful reads are cached, and only when the key did not move while the facts were read; an entry older than
600 s (`BOX_FACTS_CACHE_MAX_AGE`) is not trusted. An entry is one file with the facts, the key and the moment the
read began, renamed into place inside the lock of the task (`locks/facts-<task>.lock`) after checking that the
host's part of the key still holds and that no later read has published meanwhile. The container and result parts
of the report (agent state, queue, results, `taken`) are never cached.

### `box log <task> --json`
```json
[ { "sha": "<40>", "subject": "...", "time": "...", "filesChanged": 2, "insertions": 30, "deletions": 4 } ]
```
Commits ahead of the base, newest first, at most 200.

### `box result-log <task> <verb>`
Raw text: the last 400 lines of the log of the last result for that verb. Exit 3 when there is none.
The log of a job is kept up to 16 MiB (`BOX_MAX_LOG_BYTES`). What a job prints beyond that is dropped while the
job goes on, and the last line of the log says how many bytes were dropped. A log that reached the limit is not
parsed: `tests` is `null` (which says nothing about the checks: see `tests` above). Reading the counts out of a log has a deadline of
20 s; past it they are unknown.
Where the values of a result come from. The code of the commit runs inside the job container as the same user
as the wrapper (`image/runner/job.sh`), so nothing the wrapper could write or print there is trusted, and the
wrapper reports nothing. Produced on the host:

| Field | Source |
|-|-|
| `id`, `task`, `verb`, `sha`, `started`, `finished` | the runner (queue file written by the host, host clock) |
| `seconds` | the runner's monotonic clock around the run of the job container; the whole request when no job container ran |
| `peak_memory_bytes` | the highest of the readings the runner takes through Docker (`docker stats`) every 5 s (`BOX_MEMORY_SAMPLE_SECONDS`) while the container runs; a sampled lower bound, rounded as Docker prints it; 0 means no reading (unknown) |
| `exit_code` | the exit status of the job container as Docker reports it (128 plus the signal when a signal ended it; 124 when the runner's deadline removed it) |
| `timed_out` | exit code 124 and the runner's own measure of the run is at least the verb's time limit |
| `oom_killed` | Docker's `State.OOMKilled` |
| `cancelled` | the queue file |
| `changesChecks`, `checksTouched`, `testsTouched`, `checksReason` | the sandbox comparison of verified objects, with the host's conservative defaults |

Necessarily from the repository's own code: the verb's exit status (the commit's tests decide whether they pass;
it reaches the result only as the container's exit status) and the text of the log, from which only the test
counts (`tests`) are read, under the rules above and marked `"source": "log"`: they are reported by the code
of the commit and prove nothing about it. A line in the log or a file in the container or the checkout
that looks like a record of the runner changes nothing.

### `box journal --json [--since <time>]`
```json
[ { "at": "...", "event": "new | agent-start | agent-exit | verify-queued | verify-done | cancel | take | rm",
    "task": "m1", "verb": "test", "sha": "<40>", "exitCode": 0, "account": "personal" } ]
```
Read from `$AGENTBOX_HOME/journal.jsonl`, which only `box` appends to, one line per action it performed
(fields that do not apply are omitted). Append with a lock; a line is written after the action succeeded.
`agent-exit` is recorded by whichever `box status`, `box facts` or `box attach` call first sees the container
exited. Newest last. At most the last 2000 events.

### `box accounts --json`
```json
[ { "name": "personal", "where": "box", "loggedIn": true, "authMethod": "claude.ai", "checkedAt": "2026-11-10T08:00:00Z" } ]
```
Every element has `where`. The accounts of the boxes come first and are described here; the accounts of the Mac
follow them (see "Lavori on the Mac", "Accounts").
An account exists when its volume `agentbox-acct-<name>` exists. `loggedIn` is what `claude auth status --json`
reports inside a confined container (internal network and proxy only, no capabilities, read-only root) with
that account volume as the config directory; `authMethod` is the reported method (letters, digits, dot,
hyphen; otherwise null). Anything else the status prints is discarded, never shown or copied. Accounts are
checked concurrently with a deadline of 20 s each (`BOX_ACCOUNT_CHECK_SECONDS`); a check that fails or
times out counts as logged out and its container is removed. `checkedAt` is the UTC time of the check. About
2 s for one account.

## Projects

A project is WHAT is worked on: a repository on this Mac, an optional subfolder, the default base branch, the
install command, the commands of the checks (verbs), the protected paths and the test file pattern. A preset
(what KIND of box: limits, network, image, instructions) is a separate thing and is not built yet. Nobody edits a
file to work on another repository or folder: the app asks the person for a folder, shows what
`box project detect` proposes, and adds the project once with `box project add`. A new task names a project and
optionally a base branch.

A project is a file of the data directory, `$AGENTBOX_HOME/projects/<name>.env` (host-private), written by `box`.
It holds `KEY=value` lines (`REPO`, `BASE`, `SUBDIR`, `INSTALL`, `REBUILD`, `SPARSE`, `PROTECTED`, `TEST_FILES`,
`VERB_<verb>`, and for the place of its lavori and the lavori on the Mac `WHERE`, `BRANCH_PREFIX`, `SETUP`,
`ARCHIVE`, `ACCOUNT`, `RUN`, `PORTS`, `RUN_MODE`) and is read as data: nothing in it is evaluated. A file with a key that is not known or a value
that is not safe (a repository that is not an absolute path, a subfolder that climbs out, a base that is not a
branch name, a control character, a value over 2000 characters) is refused wherever it is used, with one `box:`
line that names the project and the reason. Commands are text: for a task in a box they only ever run inside a
container; for a lavoro on the Mac they run natively, as the person (see "Lavori on the Mac"). A task keeps the
project values it was created with, except native Play reads current `RUN`, `PORTS` and
`RUN_MODE` on each start. `SUBDIR` and
`INSTALL` may be empty: a project at the root of its repository works, and one without an install command has no
dependency volume, no install container and no `node_modules` mount. The runner prepares dependencies for jobs
only from `package.json` and `package-lock.json`.

Upgrade: the profiles shipped in `profiles/` of the code are not read as projects. The first time a data
directory has no `projects/` yet, each shipped profile whose repository is on this Mac becomes a project
(`sample-app`), except `selftest`, which becomes one when something names it (`box new s1 selftest`, or an
existing task of it). That happens once per name (`projects/.imported`): a project the person removed or
changed is not imported again. Existing tasks keep working; their records call the project `PROFILE`.

Where a repository may be. `box` runs git on the host only in a repository of the person, through one wrapper
(no hooks, no file system monitor, no pager, no external diff, no `ext` protocol, no system configuration, none
of the caller's `GIT_` variables). A folder of the data directory (the workspaces agents write) is never a
repository: that is decided by device and inode of the folder and of each of its ancestors against the data
directory and its parts, not by comparing paths, so another letter case, a link or another name of the volume
change nothing. The folder and the git directory above it must belong to the current user. The check runs when
a project is detected, added or changed and again every time a project is loaded (a folder can be replaced by a
link later).

### `box project detect <path> --json`
Proposes a project from a folder the person chose. It writes nothing.
```json
{ "repo": "/home/me/code/shop", "subdir": "apps/web", "name": "web",
  "currentBranch": "feature/cart", "defaultBranch": "main", "branches": ["feature/cart", "main"],
  "install": { "command": "npm ci", "reason": "package-lock.json is in the folder" },
  "checks": [ { "verb": "test", "command": "npm test", "reason": "script \"test\" of package.json: vitest run" } ],
  "protected": [ { "pattern": "package.json", "reason": "names the scripts and the dependencies" } ],
  "testFiles": [ { "pattern": "**/*.spec.ts", "reason": "12 tracked file(s) match" } ],
  "notes": [] }
```
`repo` is the root of the working tree (physical path), `subdir` the folder relative to it (empty at the root),
`name` a name made from the folder's name (lowercase letters, digits, hyphens) that no project has yet.
`currentBranch` is `null` on a detached head; `defaultBranch` is the branch the remote calls default when it
exists locally, else `main` or `master`, else the current one; `branches` are up to 30 local branches, most
recently committed first. `install` is `null` when there is nothing `box` can prepare. Checks come from the
`package.json` scripts `test`, `build`, `lint`, `typecheck` and `check`, run with the package manager of the
lockfile (`npm`, `pnpm`, `yarn`, `bun`; a `ng test` script gets `--watch=false`), from `Cargo.toml` (`cargo test`,
`cargo build`) and from the `test` and `check` targets of a `Makefile`. `protected` and `testFiles` are proposed
for what exists in the folder. `notes` say where a proposal would not work as it is: a lockfile other than
`package-lock.json` (no install command is proposed), a toolchain the image of the boxes does not have (`cargo`,
`make`), nothing recognised. Exit codes: 0; 3 the path is not a folder; 4 it is not inside a git repository (or
git names a root that is not the folder or above it); 5 it is a bare repository; 6 it is inside the data
directory of `box`, or its git directory is not plain (see "What a repository must be" below; the line says
which rule and how to fix it); 7 the folder or its git directory is not owned by the current user; 1 anything else (a
subfolder path with characters `box` does not accept). Each with one `box:` line on stderr and nothing on stdout.

### What a repository must be
`box` runs git on the host only in a repository of the person, and git reads more than the folder it is started
in. So before git runs, at `detect`, at `add` and `set`, and every time a project is used afterwards, the
repository is checked without git, and anything that could lead git elsewhere is refused with one `box:` line
that says why:
- the folder, its git directory and (for a linked worktree) the common directory belong to the current user and
  are outside the data directory of `box`, compared by device and inode, never by path;
- `.git` is a directory, or the pointer file of a linked worktree; never a symbolic link;
- in the git directory and the common directory, `config`, `config.worktree`, `HEAD`, `packed-refs`,
  `commondir`, `gitdir`, `objects`, `objects/info`, `refs`, `info` and `worktrees` are the plain files and
  directories git writes, never symbolic links;
- `objects/info/alternates` does not exist (the file is not read: a repository that borrows objects is refused
  whatever it borrows from; `git repack -a -d` in it and removing the file fixes it);
- the repository's own configuration has no `include` or `includeIf` section;
- the repository is found in git's own order (at each level from the folder up, a `.git` entry first, then the
  layout of a bare repository), and a folder that offers both is refused;
- after git ran, the git directory and common directory it reports are the ones that were checked.
A normal clone and a linked worktree of one (`git worktree add`) pass. What cannot be read or is not expected
is refused, not skipped.

### `box project add <name> --repo <path> [options]`, `box project set <name> [options]`, `box project rm <name>`
Options: `--subdir <dir>`, `--base <branch>`, `--install <command>`, `--check <verb>=<command>` (repeated),
`--protected <globs>`, `--test-files <globs>`, and `--where mac|box`, `--branch-prefix <prefix>` (the start of a
branch name: letters, digits, dot, hyphen, underscore, slash), `--setup <command>`, `--archive <command>`,
`--account <name>`, `--run <command>`, `--ports <1 to 100>`, `--run-mode one|many`. `add` refuses a name in use and validates everything before it
writes the file: the repository must be the root of a working tree of the person (same rules and checks as
`detect`), the subfolder a folder inside it, the base a local branch (default: the repository's default branch),
verbs `^[a-z][a-z0-9-]*$` and each given once, values without control characters. An unknown option or an option
without a value is an error. `set` changes the fields that are named and validates the whole project again; an
empty command (`--check lint=`) removes a check, an empty `--install` the install command; `--repo` and
`--subdir` are refused while tasks use the project. `rm` refuses while a task uses the project. Each prints one
line on stdout and exits 0, or one `box:` line on stderr and exits 1. A project without checks is allowed.

### `box projects --json` (also `box profiles --json`)
```json
[ { "name": "web", "repo": "/home/me/code/shop", "subdir": "apps/web", "base": "main", "where": "mac",
    "branchPrefix": "sakura/", "account": "personal", "setup": "npm ci", "archive": null,
    "run": "npm run dev -- --port $SAKURA_PORT", "ports": 10, "runMode": "one", "install": "npm ci",
    "checks": [ { "verb": "test", "command": "npm test" } ],
    "protected": "package.json package-lock.json", "testFiles": "**/*.spec.ts",
    "tasks": ["m1"], "error": null } ]
```
Every project, sorted by name. `where`, `branchPrefix`, `ports` and `runMode` always have a value (the defaults
`mac`, `sakura/`, `10`, `one` when the project does not say); `account`, `setup`, `archive`, `run`, `install`,
`protected` and `testFiles` are `null` when not set; `tasks` are the
tasks that use the project. A project that cannot be used is still listed: `error` says why (its repository does
not exist, is inside the data directory or is not owned by you), and a file that is refused as a whole is
`{"name": "...", "error": "..."}`. `box profiles --json` used to print a list of names; it is now this list.

### `box branches <project> --json`
```json
{ "project": "web", "default": "main",
  "branches": [ { "name": "feature/cart", "sha": "<40>", "time": "2026-11-10T08:00:00Z" } ] }
```
The local branches of the project's repository a task can start from (`box new --base`), most recently committed
first, at most 200. `default` is the project's base.

## Action commands

- `box new <task> <project> [--base <branch>] [--where mac|box]`: the place is the one named, else the
  project's, else `mac`; what follows describes a task in a box, see "Lavori on the Mac" for the other place.
  The project is required (there is no default project in the
  code any more). Without `--base` the task starts from the project's base branch; with it from that local
  branch of the project's repository, which must exist (one line names `box branches` otherwise). The task
  records its project, its base branch and the commit of that branch; `base` and `baseSha` of the task show
  them. Any other argument, also an unknown option such as `--preset`, is an error. The app shows its stderr on
  failure.
- `box verify <task> <verb> --no-wait`: queue the request and return. Exit 0 once queued. Queuing the same
  task, verb and sha twice stays one request.
- Before a check of a task in a box and before `box take`, the paths of the commit's tree are examined for
  the filesystem of the person: two paths that differ only in letter case or in how an accented letter is
  composed (one file on a Mac, two on the Linux of the check), a name some filesystem takes for `.git`
  (`.GIT`, `git~1`, `.git` with ignored characters, a trailing dot or space) and a name with a colon are
  refused. The check ends with exit code 2 and the line "the commit was not checked: <why>" in its log;
  `box take` ends with exit 1 and "task '<task>' was not taken: <why>. Nothing was changed in <repo>". A tree
  that cannot be examined is refused the same way.
- `box take <task>`: prints the taken sha as its last stdout line. The commit is read from the workspace once;
  the bundle is made for exactly that commit from the verified store, and after the fetch the branch
  `box/<task>` of the source repository must be at that commit, or take fails without a journal event. An agent
  that moves its branch while take runs gets the commit that was read taken, not the other one.
- `box rm <task> [--force]`: refuses while commits are not taken or files are dirty (not with `--force`), and
  for no other reason. Requests never keep a task alive: once the removal is decided, the task is marked as being
  removed inside the queue lock (`tasks/<task>.retiring`), where requests are published. From then on the intake
  of drops skips the task and `box verify` refuses it ("is being removed"), so a box that keeps asking for checks
  cannot feed the queue. The requests the task still has are then cancelled (a running one is stopped and waited
  for, as `box cancel` does) and the task goes with its record, drop directory, results and mark. If the runner
  has not ended a running check in time, `box rm` stops with one line and a second `box rm --force` finishes. A
  request an older `box` left behind for a task that no longer exists is thrown out by the runner without a
  result, and `box new` refuses that task name while such a request is in the queue.
- No phase of a request waits without a limit, also when Docker stops answering. The install and the job have
  a deadline (the verb's time limit plus a grace period); when it passes the runner asks Docker to remove the
  container, for at most the time limit of a Docker call, and then ends the docker client it was waiting for,
  first asking, 5 s later killing, whether or not Docker answered. The checkout, the package files, the import of
  the commits and the copy of the dependencies (`BOX_DEPS_MODE=copy`, 600 s, `BOX_COPY_TIMEOUT`) each have their
  own limit.
- A result is final. `results/<task>/<id>.json` is linked into place once and never replaced: not by a later
  run, not by a cancel. When the runner starts after a crash and finds a request left `running` whose result
  file already exists for the same task, verb and commit (the crash came between the result and the removal of
  the queue file), it removes the leftovers and the queue file and runs nothing; the same holds for a queued
  request whose id already has a result (the same commit and verb asked again within one second). The log of a
  published result is not truncated.
- `box verify <task> <verb>` (waiting form) never waits forever. It publishes its request through the queue for
  the commit and the incarnation of the task it read, then waits. Exit codes of verify itself: 75 the runner is
  not running (checked before the request is made, and again while waiting: on the host the lock of the runner
  is looked at; the `verify` an agent runs in its container looks at `results/<task>/.runner-alive`, which the
  runner refreshes every 5 s, older than 60 s counts as stopped), 76 the request was quarantined (for the
  agent's `verify` also its drop; reason on the line), 77 the overall deadline passed (`BOX_VERIFY_WAIT`,
  default 4500 s). Each ends with one line on stderr. Any other code is the verb's own.
- `box runner --install` / `box runner --uninstall`: the runner as a LaunchAgent (`com.agentbox.runner`) of
  the current user, so it starts at login and is restarted after a crash (not after a clean exit; throttled to
  once per 30 s). `--install` writes `~/Library/LaunchAgents/com.agentbox.runner.plist` from
  `launchd/com.agentbox.runner.plist.template` (this repository's `bin/box`, `AGENTBOX_HOME`, the `PATH` of the
  calling shell so that docker and jq are found, log in `runner.log`) and loads it with `launchctl bootstrap
  gui/<uid>`; an older copy is unloaded first. It refuses (exit 1) while a runner is running, because a second one
  would only fail on the lock: stop it first with `box runner --stop`. `--uninstall` unloads and removes it
  (exit 0, also when it was not installed) and leaves a runner started by hand alone. Unknown `--` options of
  `box runner` are refused.
- `box runner --daemon` / `box runner --stop`: start the loop detached with its log in
  `$AGENTBOX_HOME/runner.log` and its pid in the lock file; stop it by that pid. A second start refuses.

## Agent sessions

One agent container per task, named `agentbox-agent-<task>`.

- `box agent <task> <account> --detach [-- <claude args>]`: `docker run -d -it` with the usual agent flags.
  Refuses when that container already runs. An exited container of that name is removed first.
- `box attach <task>`: prints the last 2000 lines of the container's log, then `exec docker attach` with
  detach keys set to a sequence Claude Code does not use (document it here). Closing the attaching process
  leaves the agent running. Exit 3 when no agent container runs.
- `box stop <task>`: `docker stop` of that one container by name.
- `box login <account>`: interactive. One confined container whose config directory is the account volume
  runs `claude auth login` (Claude subscription, the default; the person pastes the code in the terminal),
  then `claude auth status --json`. Exit 0 only when that status says `"loggedIn": true`. Otherwise the
  last line on stderr starts with `box:` and says what failed, and the exit code is 1: the sign-in was not
  completed, it finished but the account is still not logged in, the status could not be read, no
  connection to the sign-in service (is the proxy running), or the container itself failed. The script is
  `image/runner/login.sh`, mounted read-only.
- `box shell <task>`: for a task in a box, interactive bash in a one-off agent container, for the person to look
  around. For a lavoro on the Mac it is the person's own login shell in the folder of the lavoro, not confined
  (see "Lavori on the Mac").

### `box acp <task> <account>` (the conversation view; for a lavoro on the Mac see "Lavori on the Mac")

Starts the account's ACP adapter (`claude-agent-acp`, pinned in the image, see `image/Dockerfile`) in a container
named `agentbox-acp-<task>` and connects the caller's stdin and stdout to it. The client speaks the Agent Client
Protocol: newline-delimited JSON-RPC, one message per line.

- stdout carries only messages of the adapter. `box` prints nothing there, before or after.
- `box` is the boundary between the agent and the client, and does not rely on the client being careful. A
  filter on the host (`lib/acp_proxy.pl`, perl of the system) reads both directions line by line:
  - From the client: the `initialize` request loses every entry of `clientCapabilities` except `session`. `fs`,
    `terminal`, `auth`, `elicitation`, `_meta` and anything unknown are removed, so the agent is never told that
    the client would read or write files, run commands or run a login for it.
  - From the agent: responses pass; of notifications only `session/update`; of requests only
    `session/request_permission`. Any other request (`fs/*`, `terminal/*`, anything unknown) is answered to the
    agent with the JSON-RPC error `-32601` and never reaches the client. Other notifications (also the adapter's
    `_auth/status_update`) and lines that are not a JSON-RPC object are dropped.
  - What passes is written again from what the filter parsed (same meaning, key order and escapes may differ), so
    the client reads exactly the message that was judged.
  - A line longer than 8 MiB (`BOX_ACP_MAX_LINE_BYTES`) from either side is neither parsed nor kept: the session
    is ended, the container removed, exit code 65.
  - Nothing is waited for without a limit. An agent that is still there 10 s after the client closed its side
    or a signal asked `box acp` to end (`BOX_ACP_END_SECONDS`; the deadline starts at that moment and nothing the
    agent sends or reads moves it), or a side that has not taken a byte of what waits for it for 120 s
    (`BOX_ACP_STALL_SECONDS`; the client that stopped reading, or the agent), ends the session: exit code 66. A
    session in which nobody speaks is not a stall. However a session ends, `box` removes the container by name
    afterwards, with the time limit of every other Docker call.
  - Every refusal is a line in the log, and the last line of the log counts them, by method name for the first
    20 names and as one number for the rest. That line and the line that says the log was cut are written inside
    the log's limit (2560 bytes of it are kept for them), so the log never exceeds it, whatever the agent sends.
- stdin is passed through without a TTY (`docker run -i`). When stdin closes the adapter ends, the container is
  removed (`--rm`) and `box` exits. A `TERM`, `INT` or `HUP` sent to `box` is passed on to the adapter; a kill of
  the whole process group closes stdin, which ends it too.
- The adapter's stderr goes to `$AGENTBOX_HOME/acp/<task>.log` (host-private, one session, overwritten by the
  next). It is text to read, never to execute. At most 4 MiB of it are kept (`BOX_ACP_MAX_LOG_BYTES`, the same
  byte limit the runner puts on job logs); the rest is dropped while the session goes on and a line in the log
  says how much. The last line that `box` reports is read from the last 4096 bytes of the log.
- Confinement is that of `box agent`: the task's own network and the proxy, the workspace at `/work/<task>` as the
  working directory, the task's own configuration volume `agentbox-cfg-<task>.<account>` at `/acct`, of the account
  only its credential files, 2 CPUs, 3 GB, no capabilities, read-only root. No credential passes through the
  arguments or the environment.
- An account that is not signed in (no credential file, or no account volume at all) starts all the same, without
  a credential mount; nothing is created for it. What the pinned adapter (0.103.0) then does was observed live:
  `initialize` answers (with an empty `authMethods`), `session/new` answers WITH a session, and the first
  `session/prompt` is refused with the JSON-RPC error `-32000` ("Authentication required"). That error is how
  a client learns that the account is not signed in: the adapter also sends a notification
  `_auth/status_update` ("Not logged in") right after `session/new`, which the filter below does not pass.
  After `box login <account>` the conversation must be started again to see the credential.
- One front end per task at a time, because the terminal agent and the conversation share the session history of
  the task. `box acp` refuses while `agentbox-agent-<task>` runs or another conversation is open; `box agent`
  refuses while a conversation is open. `box stop <task>` ends whichever of the two runs. The exclusion is one
  lock per task (`locks/frontend-<task>.lock`), taken before the checks and held until the process that took it
  ends: the whole session for `box acp` and for an attached `box agent`, the start for a detached agent, whose
  running container then keeps the others out. Two starts at the same moment cannot both pass.
- Exit codes: 0 the adapter ended cleanly after stdin closed; 1 `box` refused to start it (unknown task, invalid
  account name, Docker not reachable, another front end open); any other code is the adapter's own or Docker's
  (125 to 127 when the container could not be started, 128 plus the signal number when a signal killed it); 65 a
  line longer than the limit ended the session; 66 a deadline ended it. A `TERM` passed on to the pinned adapter makes it end by itself:
  observed live, the container was removed and `box` printed nothing, which is exit 0. Every non-zero exit ends with one line on stderr that starts with `box:`; for a failed adapter it carries
  the last line of the adapter's log, as plain text of at most 200 characters.

## Workspace path inside containers

The workspace is mounted at `/work/<task>` and that is the working directory (not `/work`). Reason: Claude
Code keys its session history by working directory, and two tasks on the same account must not share one
history. `rules/agent.md` says "your clone is the current directory".

## Isolation between boxes

- Network: every task has its own internal Docker network `agentbox-net-<task>` (created when its first
  container starts, attached to the proxy only). Agent, shell, install and job containers of a task run on it. A
  box cannot reach a port another box opens, by address or by name; the proxy is the one thing every task network
  has in common. `box rm` detaches the proxy from the network and removes it. Account helpers that belong to no
  task (`box login`, the checks behind `box accounts`) use the shared network `agentbox-net`. Containers started
  before this change keep running on the shared network until they are started again.
- npm cache: one volume `agentbox-npm-cache-<task>` per task, mounted at `/npm-cache`, removed with the task. On
  a real lockfile of a web application a cold `npm ci` (empty cache, through the proxy, 2 CPUs) took 11 s and a warm one
  9 s, and the cache is 112 MB per task, so the cost of not sharing it is about 2 s and 112 MB per task.

### Account configuration (one task, one configuration directory)

A box used to mount the whole account volume at `/acct`, so it could read the other tasks' conversation history
and write settings or hooks that ran in their sessions. Now `box agent <task> <account>` mounts:

- `agentbox-cfg-<task>.<account>` at `/acct`: the task's own `CLAUDE_CONFIG_DIR=/acct/claude` and
  `CODEX_HOME=/acct/codex` (settings, history, state). Created on first use with the `agentbox.task=<task>` label,
  with `claude/` and `codex/` owned by the agent user, and removed by `box rm`. Task and account are joined by a
  dot, which neither name can hold, so two tasks never get one volume (with a hyphen, task `a-b` with account `c`
  and task `a` with account `b-c` shared `agentbox-cfg-a-b-c`). A volume created under the older hyphen name is not
  used again; `box rm` still removes it by its label.
- Only the credential files of the account, each as one file mounted over it
  (`--mount type=volume,src=agentbox-acct-<account>,dst=/acct/claude/.credentials.json,volume-subpath=claude/.credentials.json`,
  and `codex/auth.json` when the account has one). The existence of the files is tested before the start
  (never their content); without `claude/.credentials.json` the start is refused: "has no login yet".

`box login` and the status check behind `box accounts` still mount the whole account volume: they belong to no
task and run one fixed CLI command. Tasks that ran before this change start with an empty configuration
directory (their old history stays in the account volume, where no box sees it any more).

What was observed (Docker 29.9.3, image `agentbox:1`, Claude Code 2.1.321, dummy files only, no real credential was
read):

| Question | Evidence | Result |
|-|-|-|
| Can one file of a volume be mounted into a box? | `volume-subpath` on a file, read and write inside two containers | Yes. A write in place is seen by the other container and lands in the account volume |
| Does a rename onto the mounted file work? | `mv`/`os.rename` over it | No: `EBUSY` (errno 16) |
| Does the CLI survive that? | Strings of the shipped binary: it writes `.credentials.json` through a staging file and a rename, and when the rename fails with `EXDEV`, `EPERM`, `EEXIST` or `EBUSY` it falls back to a write in place (open, truncate, write, chmod 600, flush) | Yes, by reading the code |
| Does the CLI follow a symlink to a shared file? | The same code opens the file with `O_NOFOLLOW` for reading and writing and reports `refused-symlink` | No: a symlink cannot be used |
| Would a copy per task work? | Refresh tokens rotate | No: the copies diverge and log the other boxes out |
| Do two boxes keep separate settings and share the credential? | Two tasks on one dummy account: one writes `settings.json` and a new credential file, the other sees the credential and no settings | Yes |
| Does the CLI run with the mounted file? | `claude auth status --json` in such a box | Yes, `configDirectory` is `/acct/claude` |

NOT VERIFIED, needs a real login (no account is logged in on this machine): that the real CLI performs a token
refresh through the mounted file without logging out the other boxes. Test to run once the person has signed in with
`box login <account>`:

1. `box agent t1 <account> --detach` and `box agent t2 <account> --detach` (two tasks, same account).
2. In each: `docker exec agentbox-agent-t1 claude auth status --json` and the same for t2, both `"loggedIn": true`.
3. Compare the file without reading it: `docker exec agentbox-agent-t1 sha256sum /acct/claude/.credentials.json` and
   the same in t2 must print the same digest, and `stat -c %Y` the same time.
4. After the access token has expired (hours later, or after `claude` made a request in either box), repeat
   steps 2 and 3: both boxes still logged in, digests equal to each other and different from step 3, and
   `docker exec agentbox-agent-t1 ls -la /acct/claude` shows no `.credentials.json.tmp*` leftovers.
5. `box accounts --json` still reports the account as logged in.

If step 4 fails (a box is logged out or the digests differ), the fallback is to give each task a copy of the
credentials and refresh them with a host-side lock, which logs boxes out on every refresh; do not ship that without
the person deciding.


## Storage limits of boxes

Every supervised container (check, install, ACP, sandbox and helper) uses `--log-driver none`.
Agent terminals and sign-in containers use Docker `json-file` logs with `max-size=8m` and
`max-file=2`. The proxy has the same rotation and logs warnings only. `/tmp` and ephemeral
homes have explicit byte and inode limits; Docker shared memory is limited separately.

A project can set `DISK_BUDGET_MB` through `box project add|set --disk-budget-mb <n>`
(1 to 1048576 MiB, default 20480 MiB). Each task keeps its creation-time value. Before starting
a task container, before a queued check starts and before take, box measures the workspace,
drop channel, retained drop/request quarantine, results, verified objects, npm cache, task configuration and dependency volumes.
The read-only helper counts allocated blocks and distinct filesystem entries, never reads
file contents, and never mounts account volumes. It refuses an operation above the byte
budget or one million entries, or when measurement fails or exceeds its deadline.

`facts` and `status` add `disk` for boxes:
`{"bytes":123456,"entries":123,"limitBytes":21474836480,"entryLimit":1000000,"overBudget":false,"enforcement":"detection"}`.
`disk` is null for a native lavoro or when measurement failed. No Docker call is added for a
native lavoro.

Limits to know: the disk budget is detection at operation boundaries, not a quota. Docker
Desktop and OrbStack do not provide per-volume byte and inode quotas for these mounts. The
workspace, persistent task configuration, drop channel, npm cache and dependency volumes
can grow between measurements. Retained results, verified objects and pristine dependency
versions count toward the task budget and are removed with the task; box does not silently
delete a result or a verified object. Writable root layers of check and dependency-install
containers, in-flight packs and bundles, Docker account volumes and Docker's own metadata
are not covered by the task budget. A running task can still fill host or VM storage. A
filesystem quota and an external disk reserve are needed for a hard storage guarantee.

The proxy is shared by all tasks and has 64 connections in total, with no capacity reserved
per task. One task can hold the shared proxy's connections and prevent other tasks from
reaching permitted services. Per-task proxies and connection quotas are not implemented.

Each `take` reserves a private temporary directory under the data home's `bundles/`.
The bundle container mounts only that directory as writable output. Overlapping takes never
share a bundle pathname; the directory is removed when its operation ends, including an
import refusal. A process killed outright can leave its private directory for manual cleanup.

## Bundled service and first run

The app sets `AGENTBOX_HOME` to its writable application-data directory. That directory and
its children are created on demand; neither a pre-existing `$AGENTBOX_HOME` nor a writable
service installation is required. On an empty home, `projects --json` returns `[]`; register
a repository with `project add`, then create its native lavoro with `new`.

For boxes, runner scripts, agent rules and proxy resources are copied to a versioned directory
under `$AGENTBOX_HOME/runtime/` before use. Container bind mounts refer to that copy, including
the proxy allow list, so the service bundle itself need not be in a directory shared with
the Docker VM. The data directory and the person's repositories must still be accessible to
the VM. Native reads keep using the bundled scripts with Python bytecode writes disabled.

A retry after a completed check gets a fresh result id even within the same clock second.
If that timestamp is already occupied, the id's timestamp advances by a second under the
queue lock. It is an ordering key; `requested`, `started` and `finished` retain real UTC time.
Queued duplicates of the same task, commit and verb still share one request.

Storage reads hold the task's shared lifecycle lock until the Docker client ends, and check
the place again under that lock. Removal or recreation cannot replace a box by a native
lavoro between the storage guard and its mount. An existing startup's exclusive lock is reused.


## Security disclosure for public distribution

Containers isolate task execution, but shared-resource availability is not guaranteed.
Persistent task storage has a measured budget, not an enforced quota: a running task can
exhaust host or VM disk space or inodes before the next operation boundary. Docker Desktop
and OrbStack provide no per-volume quota for this design. Writable container layers and
other exclusions listed above remain outside that budget.

The shared proxy has a global 64-connection limit without per-task reservations. A single
task can monopolize those connections and deny network access to other tasks. Neither disk
availability isolation nor proxy availability isolation should be advertised as a security
guarantee. Public SECURITY.md documentation must retain these limitations.
