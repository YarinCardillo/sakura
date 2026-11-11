# Changelog

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
