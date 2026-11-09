"""Peer selection: resolving llm-d's remote_kv_source pod to its certus."""

import pytest

from certus_shmq_connector import manager as M
from certus_shmq_connector import peers as P


class FakeApi:
    def __init__(self, pods):
        self.pods = pods  # ip -> annotation (None: a pod without one)
        self.calls = []
        self.fail = False

    def annotation_by_pod_ip(self, ip):
        self.calls.append(ip)
        if self.fail:
            raise OSError("api down")
        return self.pods.get(ip)


class Clock:
    def __init__(self):
        self.t = 1000.0

    def __call__(self):
        return self.t


@pytest.fixture
def clock(monkeypatch):
    c = Clock()
    monkeypatch.setattr(P, "_monotonic", c)
    return c


class Ctx:
    def __init__(self, params, req_id="r"):
        self.req_id = req_id
        self.kv_transfer_params = params


def src(ip):
    return {"remote_kv_source": {"kv_request_id": "x", "remote_host": ip, "remote_port": 7777}}


def test_a_pod_ip_resolves_to_its_annotation_once(clock):
    api = FakeApi({"10.244.2.7": "10.0.0.102:7481"})
    r = P.PeerResolver(api)
    assert r.resolve("10.244.2.7") == "10.0.0.102:7481"
    assert r.resolve("10.244.2.7") == "10.0.0.102:7481"
    assert api.calls == ["10.244.2.7"], "cached for the pod's life"


def test_a_pod_without_the_annotation_is_asked_again_only_after_the_ttl(clock):
    api = FakeApi({"10.244.2.8": None})
    r = P.PeerResolver(api)
    assert r.resolve("10.244.2.8") is None
    assert r.resolve("10.244.2.8") is None
    assert len(api.calls) == 1, "a miss is cached too, or every request costs a call"
    clock.t += P.NEGATIVE_TTL_S + 1
    api.pods["10.244.2.8"] = "10.0.0.102:7482"  # annotated meanwhile
    assert r.resolve("10.244.2.8") == "10.0.0.102:7482"


def test_an_api_failure_never_fails_the_request(clock):
    api = FakeApi({})
    api.fail = True
    r = P.PeerResolver(api)
    assert r.resolve("10.244.2.9") is None


def test_no_credentials_means_no_resolution(monkeypatch, tmp_path):
    monkeypatch.setattr(P._KubeApi.__init__, "__defaults__", (str(tmp_path / "absent"),))
    r = P.PeerResolver()
    assert r.resolve("10.244.2.7") is None
    assert r.resolve("10.244.2.7") is None and r.lookups == 0


def test_certus_fetch_from_wins_over_the_llmd_source(monkeypatch):
    api = FakeApi({"10.244.2.7": "10.0.0.102:7481"})
    monkeypatch.setattr(M, "_PEERS", P.PeerResolver(api))
    params = {"certus_fetch_from": "10.0.0.105:7482", **src("10.244.2.7")}
    assert M._fetch_peer(Ctx(params)) == "10.0.0.105:7482"
    assert api.calls == [], "the explicit hint needs no lookup"


def test_the_llmd_source_pod_is_resolved(monkeypatch):
    api = FakeApi({"10.244.2.7": "10.0.0.102:7481"})
    monkeypatch.setattr(M, "_PEERS", P.PeerResolver(api))
    assert M._fetch_peer(Ctx(src("10.244.2.7"))) == "10.0.0.102:7481"
    assert M._fetch_peer(Ctx(src("10.244.9.9"))) is None, "an unknown pod: no fetch"
    assert M._fetch_peer(Ctx({"remote_kv_source": {"remote_port": 1}})) is None
    assert M._fetch_peer(Ctx({"remote_kv_source": "junk"})) is None
    assert M._fetch_peer(Ctx(None)) is None


@pytest.fixture
def lr(monkeypatch):
    """As test_manager's: vLLM 0.30's lookup results."""
    import importlib

    import conftest
    from certus_shmq_connector import compat

    monkeypatch.delenv("CERTUS_VLLM_VERSION", raising=False)
    conftest.build_fake_vllm((0, 30))
    monkeypatch.setattr(compat, "CAPS", compat.caps_for((0, 30)))
    yield importlib.import_module("vllm.v1.kv_offload.base").LookupResult
    conftest.build_fake_vllm((0, 20))


def test_an_llmd_named_peer_drives_the_fetch(lr, monkeypatch):
    # End to end through the manager: no certus_fetch_from, only the sidecar's
    # remote_kv_source; the run is gathered and fetched from the resolved certus.
    from test_manager import FetchRing, ShmqCertusOffloadingManager, scan, U

    api = FakeApi({"10.244.2.7": "10.0.0.102:7481"})
    monkeypatch.setattr(M, "_PEERS", P.PeerResolver(api))
    ring = FetchRing()
    mgr = ShmqCertusOffloadingManager(ring, block_size_bytes=4096)
    ctx = Ctx(src("10.244.2.7"), req_id="r1")
    assert scan(mgr, ctx, [1, 2], lr) == [lr.HIT_PENDING] * 2
    mgr.on_schedule_end(None)
    assert ring.fetches == [("10.0.0.102:7481", [(U(1), 4096), (U(2), 4096)])]
    assert api.calls == ["10.244.2.7"], "one lookup for the whole request"
