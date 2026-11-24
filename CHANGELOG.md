# Changelog

## 0.4.0 - 2026-11-18

A new window built around what needs you, and an engine that reads limits from Leeway.

- The window opens on the Inbox. The sidebar lists Inbox, Master, Lanes and Engine, then your projects with their open lanes. Tabs live in the page bar with their own back and forward, Settings opens inside the window, and Cmd-K goes to any lane, project or page and runs any action of the page in view.
- Inbox: asks are grouped by lane and say what they ask in plain words. Approve, Decline, Reply, Always approve, Resume and Stop are the only answers, each with a six second Undo. J, K, A, D, R and Return work from the keyboard. Done lists what you decided and the words you gave. The morning digest is a group in the Inbox.
- Master: one conversation with Claude or Codex, chosen in the header and remembered per project. It shows the lanes it opens and the changes it proposes as cards, takes files and images, runs `/clear`, `/model`, `/mode` and `/help` itself, dictates into the composer and comes back after a restart.
- Lanes lists every lane by state. A lane's page keeps its name, state and steps (Plan, Build, Checks, Deliver) fixed at the top and scrolls only the conversation, which opens at its end. Its column holds Details, Changes with every diff, and Files that follow the agent. New lane is one sheet, from Cmd-N, the sidebar or the command palette.
- Engine page: the lane drawn as steps with the reviews that loop on them, each editable and savable as a Preset. The built-in Presets are Solo, Lean, Standard and Verified. A running lane can switch Preset or Pace and the change applies at its next step. The page also holds Pace, the running lanes, the keeper, the phone page and Automations.
- Pace: where each account should land at its weekly reset, from Slowest (50%) to Fastest (no line). Before every step the engine asks Leeway, and a lane above the line waits or moves to another account of the same provider that you allowed for the project. A lane has its own Pace. Lanes can give priority to your own agent sessions.
- Accounts and usage come from Leeway, one reader for every app. The Mac release carries a pinned Leeway helper. With one account no account choice appears.
- The engine keeper keeps lanes running when Sakura is closed and resumes them after a restart. The phone page has a switch on the Engine page. Connectors (GitHub, Notion, Linear, feeds and your own programs) feed the morning digest, and the Master can propose Automations that you accept.
- The engine keeps every decided ask with the answer, lists a lane's changed files with their patches, and gives a lane's activity as one feed in time order, with board posts and notices in words.
- The Terminal, Git, Checks and Take views of the earlier window, and boxes, are not in this window. They stay in the `boxes` build.
- The app icon is a space grey metal mark. The animated Retino behind the sidebar turns fuchsia while something needs you.
- The README, the security notes, the engine's documents and this changelog are rewritten for lanes, the Inbox, Pace and Leeway.

## 0.3.3 - 2026-11-17

- The space grey metal app icon.
- Every word in the window is a sentence: lane tool rows, Recently and the digest's checks.
- A long spec shows that it scrolls, and the checks count agrees with the lane.
- The Master's lane facts are the engine's own.

## 0.3.2 - 2026-11-17

- A window you can start from: projects, new lanes, Settings and movable tabs.
- Waits are readable, and a second project no longer stops lanes.
- The morning digest keeps going when a link is malformed.

## 0.3.1 - 2026-11-17

- Fixes from the proofs and the last reviews: program trust, approvals only while pending, delivery records and their lock, and lane states.

## 0.3.0 - 2026-11-16

- The Master in the app, lanes that talk, the board, proposals and the morning digest.

## 0.2.1 - 2026-11-16

- Engine fixes: stale cockpit actions, cage roots, caps on outside history and the restore action.

## 0.2.0 - 2026-11-16

- The engine's cockpit, a page for what waits for you (now the Inbox) and outside agents.

## 0.1.3 - 2026-11-11

- Checks for updates: on the Mac it downloads and installs them (Sparkle); on Linux and Windows it says when a newer release exists.
- Files, folders, diffs and the git history are read inside the app, with no process started for a read: no more loading lines on every click. A box task's folder is read without following links out of it, within fixed size and memory limits.
- Following the agent shows the file it changed, even through a shell command, with the changed lines marked; diffs start open.
- One name for each state: "Needs you" holds what waits for you ("Answer" or "Review"), "Idle" holds what stands still. A taken task is marked with the flower.
- The age on a sidebar row is the task's own.
- Sign in works with no task open; boxes prepare themselves the first time; a box is the default place when an account is signed in.
- A task on the Mac runs outside the sandbox with your user's permissions, and the app says so where you choose it. Permission requests from an agent there ask for every shell command and for writes to git metadata.
- Interface strings, help text and documentation rewritten in plain technical English and Italian.
- Settings rebuilt with search and a sentence for every setting; the command palette opens with Cmd+Shift+P and stands in the middle of the window.
- The app icon on Windows and the icon sizes on Linux.
- Fixes found in an independent evaluation: Play says why it cannot start, a set-aside check says why, a stopped turn is marked, tool output without protocol tags, file views without a phantom line, and more.

## 0.1.2 - 2026-11-11

- A Settings button in the status bar: Settings no longer opens only from the menu.
- The About page of Settings stands in the middle of its panel.
- The flower fills more of the app icon.
- Looking up an account no longer leaves a write error on the conversation's error stream.

## 0.1.1 - 2026-11-11

- Show a passed project check as plain green when no comparison is configured.
- Reload tasks after editing a project.
- Correct Windows process cancellation compilation and hosted confinement workspace permissions.
- Keep Linux file watchers from following symlinks outside the data folder.

## 0.1.0 - 2026-11-11

- First release under the name Sakura.
- One window for every task of your coding agents: the conversation, the changed files and their diffs, the files and the history of the task, a terminal, and the project's checks.
- A task works on your machine, in a git worktree of your repository, or in a box: a container limited and confined by Docker, with a proxy as its only way out.
- Projects are proposed from a folder you choose: base branch, install command and checks.
- A check's result belongs to one commit, and a green result that changed the checks or the tests is never shown as plain green.
- Take brings a box's branch into your repository; nothing leaves a box any other way.
- The app carries its service and keeps its data in the system's application data folder.
- English and Italian.
- macOS 11 or later. Linux and Windows builds are produced by CI; on Windows the service runs under WSL2.
