# Security

## Reporting

Report a vulnerability privately through
[GitHub's advisory form](https://github.com/YarinCardillo/sakura/security/advisories/new). Please do
not open a public issue for it. Say what you ran, on which system, and what an agent or a repository
could do that it should not. You will get an answer within a week.

## What runs where

The window runs on your Mac and starts two kinds of program: the engine's `sakura` command, and the Master's agent
(Claude Code through its ACP adapter, or `codex`). The engine starts lanes. Each lane's agents run as your user, on
your Mac, inside the native sandbox of Claude Code or Codex that the engine configures for the run. Nothing runs in
a container in 0.4.0. The phone page, when you turn it on, is a small HTTP server the engine runs on your private
network.

## Threat model

1. Everything a lane's agents, a repository or an outside agent writes is untrusted: text, files, logs, patches and
   questions. The engine and the window treat it as data. It never becomes an instruction to the Master, and the
   window shows it without running it.
2. A lane's agent may write only in its own worktree, its repository's git folder (without its hooks, config and
   project record) and a temporary folder. It may not read the folders that hold secrets, and its network is an
   allow-list. Writing outside those places, reading a secret folder or reaching a host that is not on the list is a
   vulnerability.
3. A lane may not run the programs that reach outside its sandbox: `osascript`, `open`, `launchctl`, `docker`,
   `security`, signals and `git push`. Anything that would ask you goes to the engine, which answers from a policy
   and records every refusal. A lane that gets one of those programs to run is a vulnerability.
4. The engine's state (its home, the trust records, presets and rules) lives outside every repository. A lane that
   changes it, or a repository file that weakens a setting without being named in `relax`, is a vulnerability. Hook
   commands that a repository defines never run until you trust their exact content.
5. The Master cannot edit a project, approve, mark, accept, resume or change configuration. Making it do one of those
   is a vulnerability. A proposed automation can only run a verb from a fixed list, as an argument list, and only
   after you accept it.
6. Every change from the window or the phone page is one engine command from a fixed action table. A change that
   needs a confirmation asks first, and the confirmation works once, for one identity, one action and one target.
   Running a command outside the table, or skipping a confirmation, is a vulnerability.
7. The phone page answers only a device on your private network whose login and node are listed in your own
   configuration, only for same-origin requests with a valid token, and never from the Mac it runs on. From it you
   can read, answer asks and write to a lane. Starting a lane, adding a project, changing Presets, Pace or accounts,
   and turning services or automations on are refused from it. Any way past that gate, or to one of those actions, is
   a vulnerability.
8. The window and the engine never store your passwords. Connectors keep their secrets in your Keychain, read at the
   moment of use and never printed or logged. The Leeway helper reads usage and never credentials. Any other place a
   secret ends up in is a vulnerability.
9. The engine runs only programs it resolved itself (`git`, `gh`, `tmux`, `claude`, `codex`, `node`, the adapter and
   the Leeway helper), each an executable owned by you or root that no one else can write, outside every repository. Making it run
   another one is a vulnerability.
10. A lane runs with your permissions in the places the sandbox allows, and its agent can use your signed-in accounts
    for the models. What an agent does inside those limits is not a vulnerability in Sakura. Neither are the agents'
    own behaviour, the model providers, Leeway's own forecasts, or a person with your user account on your Mac.

## Boxes

The `boxes` build and the service in `service/` confine an agent in a container that Docker limits. They are not in
the 0.4.0 window. Their model, which holds for that build:

1. A box holds a hostile agent. Everything it writes is untrusted, and the limits on it come from Docker, never
   from its cooperation.
2. A box must not reach your home folder, your keys, the Docker socket, your machine's ports or any host that is not
   on the proxy's allow list. A way to do so is a vulnerability.
3. The host never runs git inside a box's clone; what is read from it is read by a container with no network and
   validated before use.
4. A check in a box runs on a clean checkout of one commit with read-only dependencies. A box that can change the
   recorded result of a check, or the code a check runs, is a vulnerability.
5. Nothing leaves a box except through Take, which fetches one branch.
6. No container ever mounts a task that lives on your machine.
