import json
import os
import subprocess
import sys
import threading
from pathlib import Path

import pytest

from sakura.cockpit import gate, proofs

SECRET = b"p" * 32
OWNER = gate.Identity("owner@example.com", "nMAC", "mac")
PHONE = gate.Identity("owner@example.com", "nPHONE", "phone")
TARGET = ("flow", "/work/repo", "demo")
INPUTS = {"step": "strong"}
NOW = 5_000_000.0


def issue(**over):
    fields = dict(secret=SECRET, identity=OWNER, key="a", target_key=TARGET, inputs=INPUTS, now=NOW)
    fields.update(over)
    return proofs.issue(fields["secret"], fields["identity"], fields["key"], fields["target_key"], fields["inputs"],
                        now=fields["now"])


def consume(proof, **over):
    fields = dict(secret=SECRET, identity=OWNER, key="a", target_key=TARGET, inputs=INPUTS, now=NOW + 1)
    fields.update(over)
    return proofs.consume(fields["secret"], proof, fields["identity"], fields["key"], fields["target_key"],
                          fields["inputs"], now=fields["now"])


def test_ttl_is_two_minutes():
    assert proofs.PROOF_TTL_SECONDS == 120


def test_a_matching_proof_is_accepted_once():
    proof = issue()
    assert consume(proof) is True
    assert consume(proof) is False


def test_every_proof_is_different():
    assert issue() != issue()


@pytest.mark.parametrize("change", [
    dict(identity=PHONE),
    dict(identity=gate.Identity("friend@example.com", "nMAC", "mac")),
    dict(key="s"),
    dict(target_key=("flow", "/work/other", "demo")),
    dict(target_key=("flow", "/work/repo", "other")),
    dict(inputs={"step": "power"}),
    dict(inputs={"step": "strong", "extra": "x"}),
    dict(inputs={}),
    dict(secret=b"q" * 32),
])
def test_a_proof_for_other_login_node_action_target_or_inputs_is_rejected(change):
    proof = issue()
    assert consume(proof, **change) is False
    assert consume(proof) is True  # a rejected attempt does not use the proof up


def test_inputs_order_does_not_matter():
    proof = issue(inputs={"role": "builder", "model": "claude:opus"})
    assert consume(proof, inputs={"model": "claude:opus", "role": "builder"}) is True


def test_expired_and_future_proofs_are_rejected():
    proof = issue()
    assert consume(proof, now=NOW + proofs.PROOF_TTL_SECONDS + 1) is False
    assert consume(issue(now=NOW + 60)) is False
    assert consume(issue(), now=NOW + proofs.PROOF_TTL_SECONDS) is True


@pytest.mark.parametrize("proof", ["", ".", "..", "a.b.c", "1.2", "5000100.nonce.é", "x" * 5000, "1.2.3.4"])
def test_garbage_is_rejected_not_an_error(proof):
    assert consume(proof) is False


def test_tampered_expiry_is_rejected():
    expiry, nonce, mac = issue().split(".")
    assert consume(f"{int(expiry) + 1000}.{nonce}.{mac}") is False


def test_two_threads_race_for_one_proof_and_one_wins():
    proof = issue()
    results = []
    threads = [threading.Thread(target=lambda: results.append(consume(proof))) for _ in range(8)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    assert sorted(results) == [False] * 7 + [True]


def test_consumed_nonces_are_forgotten_after_expiry():
    far = 90_000_000.0
    assert consume(issue(now=NOW)) is True
    assert consume(issue(now=far), now=far + 1) is True
    assert proofs.consumed_count() == 1


# -- the persisted store: one secret and one set of used nonces for every process ----------------

LIB = str(Path(__file__).resolve().parents[1] / "lib")
CONSUME_IN_ANOTHER_PROCESS = """
import sys
sys.path.insert(0, sys.argv[3])
from sakura.cockpit import gate, proofs
who = gate.Identity("owner@example.com", "nMAC", "mac")
print(proofs.consume(proofs.load_secret(), sys.argv[1], who, "a", ("flow", "/work/repo", "demo"),
                     {"step": "strong"}, now=float(sys.argv[2])))
"""


def consume_elsewhere(proof):
    done = subprocess.run([sys.executable, "-I", "-c", CONSUME_IN_ANOTHER_PROCESS, proof, str(NOW + 1), LIB],
                          env=os.environ, capture_output=True, text=True, check=True)
    return done.stdout.strip() == "True"


def test_secret_is_created_once_private_and_shared():
    first = proofs.load_secret()
    assert len(first) == 32 and proofs.load_secret() == first
    state = Path.home() / proofs.STATE_DIRECTORY
    assert state.stat().st_mode & 0o777 == 0o700
    assert (state / proofs.SECRET_FILE).stat().st_mode & 0o777 == 0o600


def test_used_nonces_are_kept_in_a_private_file_under_the_home():
    secret = proofs.load_secret()
    assert consume(issue(secret=secret), secret=secret) is True
    path = Path.home() / proofs.STATE_DIRECTORY / proofs.NONCE_FILE
    assert path.stat().st_mode & 0o777 == 0o600 and len(json.loads(path.read_text())) == 1


def test_one_time_use_holds_across_processes():
    secret = proofs.load_secret()
    proof = issue(secret=secret)
    assert consume_elsewhere(proof) is True
    assert consume(proof, secret=secret) is False
    other = issue(secret=secret)
    assert consume(other, secret=secret) is True
    assert consume_elsewhere(other) is False


def test_processes_racing_for_one_proof_have_one_winner():
    secret = proofs.load_secret()
    proof = issue(secret=secret)
    results = []
    threads = [threading.Thread(target=lambda: results.append(consume_elsewhere(proof))) for _ in range(4)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    assert sorted(results) == [False] * 3 + [True]


def test_an_unreadable_nonce_file_refuses_instead_of_forgetting():
    secret = proofs.load_secret()
    consume(issue(secret=secret), secret=secret)
    (Path.home() / proofs.STATE_DIRECTORY / proofs.NONCE_FILE).write_text("{not json")
    with pytest.raises(proofs.ProofStoreError, match="unreadable"):
        consume(issue(secret=secret), secret=secret)


def test_a_proof_for_another_source_is_rejected():
    proof = proofs.issue(SECRET, OWNER, "a", TARGET, INPUTS, now=NOW, source="cli")
    assert consume(proof) is False
    assert proofs.consume(SECRET, proof, OWNER, "a", TARGET, INPUTS, now=NOW + 1, source="cli") is True
