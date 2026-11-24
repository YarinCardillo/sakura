# Linux port: what is macOS-only and what replaces it

Planning only. No code changes come with this page. It lists every place the engine depends on macOS, the Linux
counterpart for each, what must be measured before relying on it, and a suggested order. File names are under `lib/sakura/`.

## 1. The shape of the problem

The engine is Python, Git, tmux and two agent CLIs (Claude Code, Codex). Those four already run on Linux. The macOS
dependence sits in five places: the cage (Seatbelt), background services (launchd), secrets (Keychain), where files
live (`~/Library`), and a handful of small system tools and paths. The desktop app and the Leeway helper are separate
products and are out of scope here; the engine only has to find and talk to them.

## 2. The list

| Piece | Where | macOS today | Linux counterpart |
|-|-|-|-|
| Cage for exec roles and contract checks | `flow/cage.py` (`seatbelt_profile`, `RunCage.wrap`, `SEATBELT`), `flow/checks.py`, `flow/evidence.py`, `flow/rows.py` | `sandbox-exec -f sandbox.sb <cmd>`, default-deny reads, writes only to named folders, per-port loopback | `bwrap` (bubblewrap) with a mount namespace: read-only bind mounts of the allowed read paths, a writable bind of the run tmp and common dir, nothing else mounted, `--unshare-net` plus a loopback proxy for the allowed ports. Fallback: Landlock (kernel 5.13+, network rules from 6.7) through a small launcher. `RunCage.wrap` keeps its signature and returns the `bwrap` argv. |
| Claude Code lane and master sandbox | `flow/cage.py` (`claude_settings`: `sandbox.filesystem`, `sandbox.network`) | Claude Code applies Seatbelt itself from `settings.json` | Claude Code applies bubblewrap (and a socat proxy) itself from the same `settings.json` keys. No engine change in the keys; install `bubblewrap` and `socat`, and keep `failIfUnavailable: true` so a missing one stops the run rather than weakening it. |
| Codex lane sandbox | `flow/cage.py` (`CODEX_PROFILE`, `CODEX_PLATFORM_DENIALS`, `CODEX_DENIED_DEVICES`, `codex_args`) | Codex applies Seatbelt from the `permissions.<profile>` config the engine passes with `-c` | Codex applies bubblewrap plus seccomp (Landlock where available). The profile keys are the same; the platform denial lists (`/Applications`, `/Library/Preferences`, `/private/etc`, `/private/var/db`, `/dev/tty*`) become Linux paths (`/etc`, `/var`, `/proc`, `/sys`, `/dev/pts/*`, `/dev/tty*`). |
| Platform path lists inside the cage | `flow/cage.py` (`TOP_LEVEL_READ_DENIALS`, `OUTSIDE_HOME_READ_DENIALS`, `SYSTEM_READ_BACK`, `TOOLCHAIN_HOME_PATHS`, `HOMEBREW_TOOL_FOLDERS`, `DATA_VOLUME`, `with_aliases`) | `/private/var`, `/private/etc`, `/Library/*`, `/opt/homebrew`, the data-volume alias `/System/Volumes/Data` | Per-platform tables selected once: `/usr`, `/lib`, `/lib64`, `/bin`, `/etc` minimum set, `/proc/self`, `/dev/null|urandom|zero`, `/nix/store` or distro toolchain roots; no data-volume alias, so `with_aliases` returns its input unchanged. `/tmp` is not a link to `/private/tmp`. |
| Denied programs and Mach services | `flow/cage.py` (`DENIED_EXECUTABLES`: `osascript`, `open`, `launchctl`, `security`; Mach-service rules for pasteboard, securityd, LaunchServices) | Denied by name and by `mach-lookup` | `DENIED_PROGRAMS` keeps its generic names and adds `xdg-open`, `systemctl`, `loginctl`, `secret-tool`, `gdbus`, `dbus-send`, `busctl`. No Mach services: the D-Bus session socket (`$XDG_RUNTIME_DIR/bus`) and `$XDG_RUNTIME_DIR` are simply not mounted in the cage, which is the same rule by omission. The X11 and Wayland sockets and the `ssh-agent` and `gpg-agent` sockets likewise. |
| Background services | `keeper_launchd.py`, `keeper_cli.py`, `digest/launchd.py`, `automations/launchd.py`, `automations/registry.py`, `docs/launchd/sakura-quota.plist.template` | `~/Library/LaunchAgents/*.plist`, `launchctl bootstrap/bootout`, `RunAtLoad`, `KeepAlive.SuccessfulExit=false`, `StartCalendarInterval` | systemd user units in `$XDG_CONFIG_HOME/systemd/user/`: a `.service` with `Restart=on-failure` for the keeper (and the Leeway serve), a `.service` plus `.timer` (`OnCalendar=`) for each digest and automation, managed with `systemctl --user enable --now|disable --now|daemon-reload`. Run `loginctl enable-linger` so units start without a login session. Introduce a `ServiceManager` interface (install, remove, status) with `Launchd` and `SystemdUser` implementations; the plist dict builders become unit-text builders behind it. `launchctl` is already injected, so the tests' fake pattern carries over. |
| Keeping the machine awake | `flow/process.py` (`CAFFEINATE`, the awake guard) | `caffeinate -i -w <pid>` | `systemd-inhibit --what=idle:sleep --who=sakura --why=... --mode=block sleep infinity` bound to the engine pid, or the logind `Inhibit` call; absent tool means no guard, as today for non-Darwin (`sys.platform` check already exists). |
| Secrets for connectors and ntfy | `connectors/kinds.py`, `flow/notify.py` (`security find-generic-password`, `ntfy_keychain`) | Keychain generic passwords `sakura-connector-notion`, `sakura-connector-linear`, the ntfy topic | Secret Service through `secret-tool lookup service <name>` (libsecret; GNOME Keyring or KWallet). Headless servers have no Secret Service: fall back to a `0600` file under `$XDG_CONFIG_HOME/sakura/secrets/` or a systemd credential (`LoadCredential=`). Put the lookup behind a `SecretStore` interface; the setting name `ntfy_keychain` becomes a store-neutral name (`ntfy_secret`) with the old key read as an alias. |
| Agent logins | `flow/cage.py` (`SECRET_HOME_PATHS`: `.claude`, `.codex`, `Library/Keychains`), `accounts/` | Claude keeps its login in the Keychain keyed by `CLAUDE_CONFIG_DIR`; the default folder must stay unset | On Linux Claude Code and Codex keep credentials in files (`~/.claude/.credentials.json`, `~/.codex/auth.json`), so the folder itself is the secret: both stay in the secret path list (they already are) and the cage must deny them as files, not rely on the Keychain. The "default folder stays unset" rule has a different reason on Linux (no keychain key) but the same effect; keep it. `Library/Keychains` and `/Library/Keychains` are dropped from the Linux table, and `~/.local/share/keyrings`, `~/.local/share/kwalletd`, `~/.gnupg`, `~/.ssh` are added. |
| Engine home and state paths | `engine_home.py` (`DEFAULT_RELATIVE = Library/Application Support/Sakura/engine`), `OPERATOR_HOME_PATHS`, the master folder (`master/spec.py`), logs | `~/Library/Application Support/Sakura/engine`, `~/Library/Logs`, `~/.local/state/sakura` | XDG: `$XDG_DATA_HOME/sakura/engine` (default `~/.local/share/sakura/engine`), config in `$XDG_CONFIG_HOME/sakura`, state and logs in `$XDG_STATE_HOME/sakura`, runtime sockets in `$XDG_RUNTIME_DIR/sakura`. `engine_home.root()` is the one function that changes; every other path derives from it. `SAKURA_ENGINE_HOME` keeps priority. Existing Mac installs are untouched: the default is chosen per platform. |
| Short socket paths | `flow/tmux.py` (`SOCKET_PATH_LIMIT = 103 if darwin else 107`) | 103 bytes | Already handled (107). Prefer `$XDG_RUNTIME_DIR` (short, per-user, `0700`) for the private tmux socket and the loopback sockets of rows. `rows_root()` under the system tmp stays valid. |
| Temp folders | `flow/cage.py` (`tempfile.gettempdir()` and `realpath`, `ROWS_ROOT`) | `/var/folders/.../T` resolved to `/private/var/...` | `/tmp` or `$TMPDIR`; the `realpath` calls already make the cage compare resolved paths. Check `/tmp` is not shared between users (use `0700` subfolders, as today). |
| Process inspection | `flow/process.py` (`PS`, `LSOF`: BSD `ps` columns, `lsof -iTCP -sTCP:LISTEN`) | BSD userland | GNU `ps` accepts a different column set (`-o pid=,ppid=,pgid=,lstart=` differs; use `etime` or read `/proc/<pid>/stat`). Prefer `/proc` for pid start time and process group, and `ss -ltnp` (or `/proc/net/tcp` plus `/proc/<pid>/fd`) for listeners; `lsof` stays as a fallback if installed. The pinned-program table (`programs.py`, `SYSTEM_TOOLS`) lists `ps`, `caffeinate`, `security`, `sh`, `lsof`; the Linux table lists `ps`, `systemd-inhibit`, `secret-tool`, `sh`, `ss`. |
| Install folders and tool search | `programs.py` (`INSTALL_FOLDERS = ~/.local/bin, /opt/homebrew/bin, /usr/local/bin`), `grants.py` messages | Homebrew prefixes | `~/.local/bin`, `/usr/local/bin`, `/usr/bin`, plus Linuxbrew `/home/linuxbrew/.linuxbrew/bin` and `~/.nix-profile/bin`. The trust rule (not writable by others, not inside a repository) is platform-neutral. |
| Installed apps the engine looks for | `usage/leeway.py` (`INSTALLED_HELPERS = /Applications/Leeway.app/...`), `cockpit/phone.py` (`CLI_CANDIDATES`: Tailscale.app, Homebrew) | `.app` bundles | `SAKURA_LEEWAY` and `programs.toml` already win. Add `/usr/bin/leeway`, `~/.local/bin/leeway`, `/opt/leeway/leeway` once a Linux helper exists; for Tailscale use `/usr/bin/tailscale`. If there is no Linux Leeway, `usage/` must return its "helper not found" state and the engine runs without usage advice. |
| Clipboard and desktop hooks | `flow/cage.py` (`pbpaste`, `pbcopy` allowed names) | Pasteboard tools | `wl-copy`/`wl-paste`, `xclip`, `xsel`. The cage denies them by omitting the display sockets; the allow list for harmless tools drops `pbpaste`/`pbcopy` and does not add the Linux ones. |
| Toolchain and system reads | `flow/cage.py` (`/Library/Preferences/com.apple.dt.Xcode.plist`, Xcode and Command Line Tools paths, `/Library/Developer`, `/Library/Frameworks/Python.framework`) | Apple toolchain layout | Compiler and interpreter roots from the distro (`/usr/lib/gcc`, `/usr/lib/python3*`, `/usr/include`, `/usr/share`), plus `~/.cargo`, `~/.rustup`, `~/.nvm`, `~/.pyenv` already in `TOOLCHAIN_HOME_PATHS`. Derive the list from `ldd` and the interpreter prefixes where possible. |
| Case-insensitive file names | Rules and tests that fold case (`.GIT` and `.git`, `Read(...)` matching) | APFS is case-insensitive by default | ext4 and btrfs are case-sensitive. Deny rules stay valid (stricter in effect); tests that expect `.GIT` to hit the Git folder must be marked macOS-only or fixture-created per platform. |
| Signal, priority and sleep behaviour | `flow/process.py` (`os.nice`, process groups, `os.killpg`) | POSIX | Works unchanged. Note `os.nice` cannot lower a niceness without privilege on either system; no change. |
| Desktop integration | `cockpit/` (serve, phone page, `host_actions`) | Loopback HTTP server, a native macOS app as client | The server is portable. A Linux client is a separate product decision (Tauri or a browser on the phone page). Nothing in the engine needs `open`, `osascript` or AppleScript; the one place that launches a terminal for a login (see the account actions) must emit a command for the app to run, never spawn a terminal itself. |

## 3. What does not change

Git and worktree handling, the ledger and board files, tmux sessions (private server on its own socket), the cockpit
HTTP server and actions, the MCP servers (`cage_server`, master server), the permission prompt server, hooks, presets,
budgets, rules and every pure-Python module. Python 3.11+ and tmux 3.x are the only runtime requirements.

## 4. Risks to measure before trusting the Linux cage

1. **User namespaces.** `bwrap` needs unprivileged user namespaces. Ubuntu 24.04 restricts them through AppArmor
   (`kernel.apparmor_restrict_unprivileged_userns`); containers and some hardened kernels disable them. The engine
   must detect this at start (run a no-op `bwrap --unshare-all true`), and refuse to run caged roles with a message
   naming the sysctl or profile, never fall back to an uncaged run.
2. **Network rule.** Seatbelt allows a loopback port per rule. `bwrap --unshare-net` gives an empty network
   namespace; per-port loopback needs a proxy or a veth with firewall rules. Reuse the proxy design Claude Code uses
   (a unix socket bridged to an allow-listed proxy) and keep `strictAllowlist`.
3. **Read default-deny is by mount, not by rule.** A path that is not mounted does not exist; error messages change
   from "Operation not permitted" to "No such file or directory". Tests that match the message must match both.
4. **Glob secrets** (`.env*`, `**/.env*`) are deny rules on macOS. A mount cannot express a glob: either bind
   `/dev/null` over each existing match at launch, or mount only the entries that are not secrets (the master already
   grants repositories entry by entry, which is the same approach). The Read-tool permission rules still apply.
5. **`/proc` and `/sys` exposure.** Mount a minimal `/proc` (`--proc /proc` with its own pid namespace) so lanes do not
   read other users' command lines or the engine's environment.
6. **Agent CLI parity.** Pin and measure the Claude Code and Codex versions on Linux: the sandbox keys, the
   `blockReadsOutsideWorkingDirectories` behaviour and `additionalDirectories` were measured on macOS builds only.
7. **Keyring availability.** Headless boxes have no Secret Service; the file fallback needs `0600` checks and the
   same "never print the secret" rule as the Keychain path.

## 5. Suggested order

1. Introduce the three seams with macOS implementations only, no behaviour change: `Sandbox` (wrap and platform
   tables), `ServiceManager`, `SecretStore`. Tests keep their fakes. Add one `platform` module that holds every
   per-OS table so `cage.py` stops holding macOS literals.
2. Move `engine_home` defaults and `OPERATOR_HOME_PATHS` behind the platform module (XDG on Linux).
3. Linux `ServiceManager` (systemd user) and `SecretStore` (secret-tool, file fallback), run on a Linux CI image.
4. Linux `Sandbox` through `bwrap` for exec roles and checks; keep Claude and Codex sandboxes to their own settings.
5. A Linux acceptance run: the existing canaries (`acceptance/`) rerun with the Linux probes (read another folder of the
   home, read a registered repository, read its `.git`, write outside the work folder, connect to a denied port).
6. Mark macOS-only tests (`sandbox-exec` probes, launchd plist content, Keychain) with a platform skip and add the
   Linux twins beside them.

## 6. Out of scope

Windows, the desktop app, the Leeway helper's own port, packaging (deb, rpm, Nix), and any change to what the
engine guarantees: every cage rule stated for macOS must hold on Linux or the role does not run.
