"""The programs and commands the Claude cage denies, refused by name in a Codex role's hook (Codex cannot deny them itself)."""

import pytest

from sakura.flow import cage, denied_commands


def _denied(command):
    return denied_commands.reason(command, cage.DENIED_PROGRAMS, cage.DENIED_COMMANDS)


@pytest.mark.parametrize("command", [
    "osascript -e 'return 1'", "/usr/bin/open -a Calculator", "ls | open .", "FOO=1 kill 1", "env pkill sleep",
    "cd x && launchctl list", "sh -c 'osascript -e x'", 'bash -lc "cd x && security list-keychains"', "ps aux",
    "time nice -n 5 killall Dock", "echo hi; pbpaste", "git push origin main", "git -C . push", "git -c a=b push --force",
    "(docker ps)", "xargs kill < pids.txt", "sudo -n tailscale up", "ls\nosascript -e 1", "OSASCRIPT -e 1", "Git Push",
    "env -u HOME launchctl list", "timeout -s KILL 5 killall Dock", 'eval "osascript -e 1"', "fish -c 'open .'",
    "ls; osa\\\nscript -e 1", "ENV -u X kill 1", "Nice -n 5 killall Dock", "env A=1 sudo -n git push", "echo \\\\\nosascript -e 1",
])
def test_Reason_ADeniedProgramOrCommandInCommandPosition_IsRefusedByName(command):
    assert _denied(command)


@pytest.mark.parametrize("command", [
    "ls -la", "echo open the door", "grep -r kill src", "git log --oneline", "git commit -m 'push it'", "git status",
    "python3 -c 'import os; print(os.getpid())'", "cat notes-about-docker.md", "echo 'a; open b'", "make open-docs",
    "git pushd", "rg security docs",
])
def test_Reason_AWordOfADeniedNameThatIsNotAProgramBeingStarted_IsLeftAlone(command):
    assert _denied(command) is None


def test_Reason_NamesWhatWasRefused():
    assert "osascript" in _denied("osascript -e 1") and "git push" in _denied("git push")


def test_Reason_ACommandThatCannotBeRead_IsLeftToTheOtherGuards():
    assert _denied("echo 'unterminated") is None


def test_Reason_AChainOfWrappersCostsABoundedAmount():
    import time
    started = time.monotonic()
    assert _denied("env -u X " * 90 + "osascript -e 1")  # too long to look through: refused, not passed
    assert _denied("env " * 400 + "osascript -e 1")
    assert time.monotonic() - started < 2


def test_Reason_NestingPastTheLimit_IsRefusedNotPassed():
    assert _denied("sh -c \"sh -c 'sh -c \\\"sh -c ls\\\"'\"")
