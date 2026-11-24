import io
import subprocess
from types import SimpleNamespace

import pytest
from sakura.digest import supervise


@pytest.fixture
def supervised_child(monkeypatch):
    """One process group with scripted pipe output and an absolute clock."""
    class Stdin(io.BytesIO):
        def close(self):
            state.input = self.getvalue()
            super().close()

    stdin = Stdin()
    class Pipe:
        pass

    pipes = (Pipe(), Pipe())
    for index, pipe in enumerate(pipes):
        pipe.fileno = lambda index=index: index
        pipe.close = lambda: None
    group = SimpleNamespace(pid=701, returncode=None, stdin=stdin, stdout=pipes[0], stderr=pipes[1])
    state = SimpleNamespace(elapsed=0.0, output={}, signals=[], launches=[], deadline=0.0, group_alive=True, seconds_per_read=0.0)

    def spawn(argv, **kwargs):
        state.launches.append((argv, kwargs))
        return group

    def ready(readers, _writers, _errors, seconds):
        waiting = [pipe for pipe in readers if state.output.get(pipe.fileno())]
        if not waiting:
            state.elapsed += seconds
            if seconds <= 0 or state.elapsed >= state.deadline + 0.01:
                pytest.fail("the supervisor continued past the total deadline")
        return waiting, [], []

    def killpg(pid, sig):
        assert pid == group.pid
        if not state.group_alive:
            raise ProcessLookupError
        if sig:
            state.signals.append(sig)
            state.group_alive = False
            group.returncode = -sig

    group.wait = lambda: group.returncode
    group.poll = group.wait
    monkeypatch.setattr(supervise, "time", SimpleNamespace(
        monotonic=lambda: state.elapsed, sleep=lambda seconds: setattr(state, "elapsed", state.elapsed + seconds),
    ))
    monkeypatch.setattr(supervise, "select", SimpleNamespace(select=ready))
    def read(fd, _size):
        state.elapsed += state.seconds_per_read
        return state.output[fd].pop(0)

    monkeypatch.setattr(supervise, "os", SimpleNamespace(read=read, killpg=killpg))
    monkeypatch.setattr(supervise, "subprocess", SimpleNamespace(
        Popen=spawn, PIPE=subprocess.PIPE, DEVNULL=subprocess.DEVNULL, TimeoutExpired=subprocess.TimeoutExpired,
    ))
    return state
