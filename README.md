# Sakura

A desk for coding agents: each task in a place of its own, and you decide what comes out of it.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/lockup-on-dark.svg">
    <img src="docs/images/lockup-on-light.svg" width="492" alt="Sakura">
  </picture>
</p>

You give an agent a task. It works in a place made for that task: a git worktree on your machine, or a
box, a container that Docker confines and limits. In one window you follow the conversation beside the
files it changes, run your project's checks on what it committed, and take the work when it holds.

Sakura runs on your machine. It has no account, no remote server and no telemetry.

## Install

**macOS 11 or later.** Download `Sakura-<version>.zip` from
[Releases](https://github.com/YarinCardillo/sakura/releases/latest), unzip it and move Sakura to
Applications. Or with Homebrew:

```sh
brew install --cask yarincardillo/tap/sakura
```

**Linux (x86_64).** Download `sakura-<version>-linux-x86_64.tar.gz`, unpack it anywhere and run
`./sakura` from the folder. It needs X11 or Wayland and a Vulkan driver.

**Windows (x86_64).** Download `Sakura-<version>-windows-x86_64.zip` and unpack it. The service Sakura
drives is a set of Unix shell scripts, so on Windows it runs under WSL2: install it with
`wsl --install`, and install the tools below inside it. For boxes, the `docker` command inside WSL
must reach your Docker daemon. Without WSL the window says so and what to do.

Sakura is made and used on macOS. CI is configured to build and test Linux and Windows; these
workflows have not yet run for this repository. See
[what is verified on each system](#what-is-verified-on-each-system) before you rely on them.

### What it needs

| For | You need |
|-|-|
| Everything | `git`, `jq`, `perl`, `python3`, `bash` (on a Mac only `jq` is missing: `brew install jq`) |
| A task on your machine | Node.js with `npm`, and Claude Code signed in. The agent's adapter is installed on first use |
| A task in a box | Docker Desktop or OrbStack, running |

Without Docker everything works for tasks on your machine: the conversation, the changes, the files,
the checks, the terminal and Play. Only boxes need Docker.

With Docker Desktop, add the folder that holds Sakura (for example `/Applications`) under Settings,
Resources, File sharing: a box mounts two read-only files of the service the app carries. OrbStack
shares it already.

Weekly usage is read from [Leeway](https://github.com/YarinCardillo/leeway) when it is installed.

## The first task

1. Open Sakura and choose **New task** (Cmd-N on macOS).
2. The first time, choose a folder of a git repository. Sakura proposes a project from it: the base
   branch, the install command and the checks it found. Change what is wrong and confirm.
3. Say what you want done, and choose whether it runs on your computer or in a box.
4. Follow the agent in **Agent**. The files it changes are listed beside the conversation;
   **Changes** shows every diff, **Files** and **Git** the tree and the history, **Terminal** a shell
   in the task's folder.
5. Run the project's checks in **Checks**, then **Take**.

A task on your machine is a worktree of your repository on a new branch `sakura/<task>`: the branch is
already yours, and publishing it is something you ask the agent or do yourself. A task in a box is a
clone; **Take** fetches its branch into your repository as `box/<task>`, and nothing leaves a box any
other way.

## How a box confines

The limits are set by Docker when the container starts, never by asking the agent to behave.

- It sees its own clone of the repository, its dependencies, the login of the one account you chose,
  and a folder where it can ask for a check. Not your home folder, not your SSH keys, not the Docker
  socket.
- 2 CPUs, 3 GB of memory, no capabilities, a read-only root file system.
- Its only way out is a proxy that allows the package registry, GitHub and the model APIs, as HTTPS
  to port 443. Plain HTTP, addresses given as numbers, other ports and your own machine are refused.
- Everything it writes is treated as hostile: the host never runs git inside its folder, and what is
  read from it is read by a second container with no network and checked before it is shown.

A task on your machine is not confined. The agent there runs as you, with your permissions: Sakura
gives it a worktree and order, not a wall. Choose a box for work you would not run yourself.

## What a check proves

A check is a command your project names (`test`, `build`, whatever you add). Its result belongs to one
commit. In a box it runs on a clean checkout of exactly that commit, with dependencies built from the
lock file and mounted read-only, where the agent cannot reach. On your machine it runs in the
worktree, one check at a time, and only when nothing is uncommitted.

A green result proves that the command passed on that commit. It does not prove that the command still
means what it meant. So a green result is never shown as plain green when the commit changes the
checks themselves, touches tests that existed at the base, runs fewer tests than the base or skips
more of them: the row says which, and **Take** then asks you to hold the button, with the reason.

On native Linux, the container user (UID 1000) needs write access to its task workspace. The hosted confinement fixture grants that UID an ACL on its throwaway workspace; Docker Desktop supplies its own host-volume mapping.

## Where things are

Sakura and its service keep everything in one data folder, created on first run:
`~/Library/Application Support/Sakura` on macOS, `$XDG_DATA_HOME/sakura` (default
`~/.local/share/sakura`) on Linux. On Windows the service's data is inside WSL, at the Linux path, and
the app's own preferences are in `%APPDATA%\Sakura`.

| Variable | Meaning |
|-|-|
| `AGENTBOX_HOME` | The data folder, in place of the default |
| `SAKURA_BOX_BIN` | The `box` command to use, in place of the one the app carries |
| `SAKURA_APPEARANCE` | `dark` or `light`; otherwise the system's, followed live |
| `SAKURA_LANGUAGE` | `en` or `it` for one launch; otherwise the system's, or the one chosen in Settings |
| `SAKURA_NO_ACTIVATE` | Do not bring the app to the front at launch |
| `SAKURA_FIXTURE` | `1` shows built-in sample tasks without calling `box` |

The app runs only the `box` command of its service. It never runs git or Docker itself and never
reads a credential. The service is documented in [service/README.md](service/README.md) and its
machine interface in [service/docs/interface.md](service/docs/interface.md).

## Uninstall

Move Sakura to the Trash (or `brew uninstall --cask sakura`). Your tasks, their worktrees and the
logins of the boxes stay in the data folder and in Docker volumes named `agentbox-*` until you delete
them.

## Building from source

Rust 1.99. On Linux also the development packages listed in
[.github/workflows/ci.yml](.github/workflows/ci.yml).

```sh
SAKURA_BOX_BIN="$PWD/service/bin/box" cargo run --release -p sakura-app
SAKURA_FIXTURE=1 cargo run -p sakura-app          # sample tasks, without the service

cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=1
bash service/tests/run.sh                          # the service's suite, with a stand-in Docker

scripts/bundle-macos.sh                            # Sakura.app, signed ad hoc
```

`crates/sakura-core` runs `box` and holds the model, `crates/sakura-acp` is the agent conversation
over the Agent Client Protocol, `crates/sakura-terminal` the terminal view, `crates/sakura-app` the
window. `service/` is the `box` command.

lavoro: a task, the Italian word the code uses for one piece of work.

## What is verified on each system

The CI entries below describe configured checks. Hosted results are available in [Actions](https://github.com/YarinCardillo/sakura/actions).
The published 0.1.0 tree passed the macOS app tests and Linux linting on hosted runners. Linux and
macOS release archives were built successfully. Windows compilation, Linux watcher and terminal
fixtures, and the real Docker fixture have pending CI fixes; their corrected runs must pass before
those checks are considered verified.
The Windows service requires WSL2 and has no hosted end-to-end runtime check. Linux and Windows
archives are built by the release workflow; producing an archive does not verify desktop use.
The macOS 0.1.0 archive was signed with Developer ID and notarised separately.

| | macOS | Linux | Windows |
|-|-|-|-|
| Format, clippy with warnings denied | CI | CI | CI (tests are not linted) |
| Tests of the core and of the conversation | CI | CI | CI, except the ones that stand a shell script in for `box` |
| Tests of the window and the terminal | CI | CI, except layout tests that need the Mac's text shaping | not run |
| The service's suite | CI | CI | not run (it needs WSL) |
| A box's confinement against a real Docker | by hand, `service/tests/confinement.sh` | CI, reported but not yet required | not run |
| The packaged app opened and used by a person | not verified for this snapshot | not yet | not yet |

## Licence

Sakura is AGPL-3.0-only and contains GPL-3.0-or-later code adapted from Zed, combined under section 13 of both licences. See [LICENSE](LICENSE). What Sakura is built from is listed in
[THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES). Packaged builds also carry the original dependency
copyright and licence texts in `THIRD_PARTY_LICENSES`. [Changelog](CHANGELOG.md),
[security](SECURITY.md), [contributing](CONTRIBUTING.md), [releasing](docs/release.md).
