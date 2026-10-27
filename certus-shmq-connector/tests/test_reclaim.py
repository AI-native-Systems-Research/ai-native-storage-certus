"""Channel reclaim in the Python ring, against the Rust shmq-echo server.

A client that dies holding channels -- SIGKILL, no exit handlers -- must not
leak them; a live one must keep them; a forked child must not keep its dead
parent's. See lib/shm-queue's module docs, "Channel claims"."""

import os
import signal
import subprocess
import sys
import textwrap
import time

import pytest

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
ECHO = os.path.join(REPO, "target", "debug", "shmq-echo")
CONNECTOR = os.path.join(REPO, "certus-shmq-connector")

pytestmark = pytest.mark.skipif(not os.path.exists(ECHO),
                                reason="needs target/debug/shmq-echo (cargo build -p shm-queue)")


@pytest.fixture
def mailbox(tmp_path):
    path = f"/dev/shm/certus-reclaim-test-{os.getpid()}-{time.monotonic_ns()}"
    srv = subprocess.Popen([ECHO, "serve", path, "4", "4096", "4096"],
                           stderr=subprocess.DEVNULL)
    from certus_shmq_connector.ring import Ring
    deadline = time.monotonic() + 10
    while not os.path.exists(path):
        assert time.monotonic() < deadline, "echo server did not start"
        time.sleep(0.01)
    yield path, Ring
    srv.kill()
    srv.wait()
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass


def py_holder(path, n, fork=False):
    """A Python process holding ``n`` channels (one per thread). With ``fork``,
    it forks a child that outlives it, and prints the child's pid."""
    code = textwrap.dedent(f"""
        import os, sys, threading, time
        sys.path.insert(0, {CONNECTOR!r})
        from certus_shmq_connector.ring import Ring
        r = Ring({path!r}, ready_timeout=5)
        held = []
        def take():
            r.request(7, b"x")   # claims this thread's channel
            held.append(r._tls.channel)
            time.sleep(600)
        for _ in range({n}):
            threading.Thread(target=take, daemon=True).start()
        while len(held) < {n}:
            time.sleep(0.01)
        child = 0
        if {fork!r}:
            child = os.fork()
            if child == 0:
                time.sleep(600)
        print("held", sorted(held), child, flush=True)
        time.sleep(600)
    """)
    p = subprocess.Popen([sys.executable, "-c", code], stdout=subprocess.PIPE, text=True)
    line = p.stdout.readline().split()
    assert line and line[0] == "held", line
    return p, int(line[-1])


def test_a_killed_python_clients_channels_are_reclaimed(mailbox):
    path, Ring = mailbox
    p, _ = py_holder(path, 4)
    r = Ring(path, ready_timeout=5)
    with pytest.raises(Exception, match="no free shmq channel"):
        r.request(7, b"me")
    p.send_signal(signal.SIGKILL)
    p.wait()
    status, payload = r.request(7, b"me")
    assert (status, payload) == (7, b"me")
    assert r.reclaimed == 1
    r.close()


def test_a_live_clients_channels_are_kept(mailbox):
    path, Ring = mailbox
    p, _ = py_holder(path, 3)
    r = Ring(path, ready_timeout=5)
    assert r.request(7, b"a") == (7, b"a")   # the one free channel
    assert r.reclaimed == 0
    p.kill()
    p.wait()
    r.close()


def test_a_forked_child_does_not_keep_its_dead_parents_claims(mailbox):
    path, Ring = mailbox
    p, child = py_holder(path, 4, fork=True)
    p.send_signal(signal.SIGKILL)
    p.wait()
    try:
        os.kill(child, 0)  # the child is still alive
        r = Ring(path, ready_timeout=5)
        assert r.request(7, b"c") == (7, b"c"), "the child kept the dead parent's claim"
        r.close()
    finally:
        os.kill(child, signal.SIGKILL)


def test_python_reclaims_a_dead_rust_clients_channel(mailbox):
    path, Ring = mailbox
    h = subprocess.Popen([ECHO, "hold", path, "4"], stdout=subprocess.PIPE, text=True)
    assert h.stdout.readline().startswith("held")
    h.kill()
    h.wait()
    r = Ring(path, ready_timeout=5)
    assert r.request(7, b"r") == (7, b"r")
    assert r.reclaimed == 1
    r.close()


def test_a_takeover_waits_out_a_dead_rust_clients_stuck_request(mailbox):
    # `hold ... stuck` publishes a request the echo server never answers: the
    # channel can never be taken over safely, so the claim must skip it.
    path, Ring = mailbox
    h = subprocess.Popen([ECHO, "hold", path, "1", "stuck"], stdout=subprocess.PIPE, text=True)
    stuck = int(h.stdout.readline().split("[")[1].split("]")[0])
    h.kill()
    h.wait()
    import certus_shmq_connector.ring as ringmod
    old = ringmod.TAKEOVER_TIMEOUT_S
    ringmod.TAKEOVER_TIMEOUT_S = 0.2
    try:
        r = Ring(path, ready_timeout=5)
        assert r.request(7, b"s") == (7, b"s")
        assert r._tls.channel != stuck, "took a channel with a request still in flight"
        r.close()
    finally:
        ringmod.TAKEOVER_TIMEOUT_S = old


def test_close_releases_every_threads_claim(mailbox):
    import threading
    path, Ring = mailbox
    a = Ring(path, ready_timeout=5)
    ts = [threading.Thread(target=lambda: a.request(7, b"t")) for _ in range(4)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    b = Ring(path, ready_timeout=5)
    with pytest.raises(Exception, match="no free shmq channel"):
        b.request(7, b"b")
    a.close()
    assert b.request(7, b"b") == (7, b"b")
    assert b.reclaimed == 0, "a clean close is not a takeover"
    b.close()


def test_release_frees_the_channel_while_the_process_lives(mailbox):
    import threading
    path, Ring = mailbox
    a = Ring(path, ready_timeout=5)
    def churn():
        a.request(7, b"t")
        a.release_channel()   # a churning worker thread, as ThreadPoolExecutor users do
    for _ in range(12):       # 12 threads through 4 channels: leaks would starve them
        t = threading.Thread(target=churn)
        t.start()
        t.join()
    b = Ring(path, ready_timeout=5)
    ts = [threading.Thread(target=lambda: b.request(7, b"b")) for _ in range(4)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    assert len(b._claim_fds) == 4, "a released channel stayed locked"
    a.close()
    b.close()


def _serve(path):
    srv = subprocess.Popen([ECHO, "serve", path, "4", "4096", "4096"], stderr=subprocess.DEVNULL)
    deadline = time.monotonic() + 10
    while True:
        try:
            with open(path, "rb") as f:
                if int.from_bytes(f.read(4), "little") == 0x5148_4D53:
                    return srv
        except FileNotFoundError:
            pass
        assert time.monotonic() < deadline, "echo server did not start"
        time.sleep(0.01)


def test_a_restarted_server_fails_an_orphaned_request_fast(mailbox):
    """The server restarts (new inode, new generation) under a live ring: its
    next request must say so within about QUIET_BEFORE_PROBE_S, not hang to the
    deadline -- an engine's 30 s deadline is what killed it before."""
    from certus_shmq_connector.ring import ServerRestartedError
    path, Ring = mailbox
    r = Ring(path, ready_timeout=5, deadline=30)
    assert r.request(7, b"a")[1] == b"a"
    old_gen = r.generation
    assert r.server_replaced() is None

    # The fixture's server, killed: SIGKILL leaves its file, so remove it as
    # its own drop would; then a new one at the same path.
    subprocess.run(["pkill", "-KILL", "-f", f"shmq-echo serve {path} "], check=False)
    time.sleep(0.2)
    os.unlink(path)
    new = _serve(path)
    try:
        assert r.server_replaced() not in (None, old_gen)
        t = time.monotonic()
        with pytest.raises(ServerRestartedError, match="restarted"):
            r.request(7, b"b")
        assert time.monotonic() - t < 5
    finally:
        new.kill()
        new.wait()


def test_a_stopped_server_is_only_a_deadline(mailbox):
    from certus_shmq_connector.ring import RingError, ServerRestartedError
    path, Ring = mailbox
    r = Ring(path, ready_timeout=5, deadline=1.5)
    r.request(7, b"a")
    subprocess.run(["pkill", "-KILL", "-f", f"shmq-echo serve {path} "], check=False)
    time.sleep(0.2)
    os.unlink(path)
    with pytest.raises(RingError, match="deadline") as e:
        r.request(7, b"b")
    assert not isinstance(e.value, ServerRestartedError)
