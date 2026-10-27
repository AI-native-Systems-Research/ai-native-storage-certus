# SPDX-License-Identifier: Apache-2.0
"""Which certus instance holds a peer pod's cache.

llm-d's routing sidecar names a source *pod* for a cached-prefix pull, in
``kv_transfer_params["remote_kv_source"]["remote_host"]`` (its pod IP). The
certus to fetch from is the one that pod's model server stores into: each model
server annotates its own pod with it, ``certus.ai/transfer-endpoint: "ip:port"``
(see ``deploy/llm-d/profiles/a30/certus/entrypoint-certus.sh``). This module
reads that annotation, by pod IP, through the Kubernetes API.

Design: ``components/certus-transfer-endpoint/specs/001-rdma-cache-transfer/
peer-selection.md``.

The lookup runs inside vLLM's scheduler step, so it must not stall it:

* answers are cached per pod IP for the pod's life -- a restarted pod has a new
  IP, so an entry never goes stale in a way that names the wrong certus;
* a failed or empty answer is cached too, for ``NEGATIVE_TTL_S``, so a pod with
  no annotation costs one API call per interval, not one per request;
* each call is bounded by ``API_TIMEOUT_S``.

Without in-cluster credentials (no service-account token: a hand-run vLLM, or a
pod whose account mounts none) the resolver answers ``None`` for everything
and says so once.
"""

from __future__ import annotations

import json
import os
import ssl
import sys
import threading
import time
import urllib.parse
import urllib.request

ANNOTATION = "certus.ai/transfer-endpoint"
SA_DIR = "/var/run/secrets/kubernetes.io/serviceaccount"
API_TIMEOUT_S = 2.0
NEGATIVE_TTL_S = 30.0

_monotonic = time.monotonic  # replaced by tests


def _log(msg: str) -> None:
    print(f"[certus-shmq] {msg}", file=sys.stderr, flush=True)


class _KubeApi:
    """The one call the resolver needs: pods in a namespace, by IP."""

    def __init__(self, sa_dir: str = SA_DIR):
        self._token = open(os.path.join(sa_dir, "token")).read().strip()
        self._ns = os.environ.get("POD_NAMESPACE") or open(
            os.path.join(sa_dir, "namespace")
        ).read().strip()
        self._ctx = ssl.create_default_context(cafile=os.path.join(sa_dir, "ca.crt"))
        host = os.environ.get("KUBERNETES_SERVICE_HOST", "kubernetes.default.svc")
        port = os.environ.get("KUBERNETES_SERVICE_PORT", "443")
        self._base = f"https://{host}:{port}"

    def annotation_by_pod_ip(self, ip: str) -> str | None:
        q = urllib.parse.urlencode({"fieldSelector": f"status.podIP={ip}"})
        req = urllib.request.Request(
            f"{self._base}/api/v1/namespaces/{self._ns}/pods?{q}",
            headers={"Authorization": f"Bearer {self._token}"},
        )
        with urllib.request.urlopen(req, timeout=API_TIMEOUT_S, context=self._ctx) as r:
            items = json.load(r).get("items", [])
        for pod in items:
            if pod.get("status", {}).get("phase") != "Running":
                continue
            ep = (pod.get("metadata", {}).get("annotations") or {}).get(ANNOTATION)
            if ep:
                return ep
        return None


class PeerResolver:
    """Pod IP -> the certus instance (``"ip:port"``) holding that pod's cache."""

    def __init__(self, api=None):
        self._lock = threading.Lock()
        self._known: dict[str, str] = {}
        self._missing: dict[str, float] = {}  # ip -> when to ask again
        self._api = api
        self._tried_default = api is not None
        self.lookups = 0  # API calls made, for tests and logs

    def _client(self):
        if not self._tried_default:
            self._tried_default = True
            try:
                self._api = _KubeApi()
            except OSError as e:
                _log(f"no in-cluster credentials ({e}); llm-d-named peers are not resolved, "
                     f"only certus_fetch_from is used")
        return self._api

    def resolve(self, pod_ip: str) -> str | None:
        with self._lock:
            if pod_ip in self._known:
                return self._known[pod_ip]
            again = self._missing.get(pod_ip)
            if again is not None and _monotonic() < again:
                return None
            api = self._client()
            if api is None:
                return None
            self.lookups += 1
            try:
                ep = api.annotation_by_pod_ip(pod_ip)
            except Exception as e:  # noqa: BLE001 - never fail a request over a lookup
                _log(f"resolving peer pod {pod_ip}: {e}")
                ep = None
            if ep:
                self._known[pod_ip] = ep
                self._missing.pop(pod_ip, None)
            else:
                if again is None:
                    _log(f"peer pod {pod_ip} has no {ANNOTATION}; not fetching from it")
                self._missing[pod_ip] = _monotonic() + NEGATIVE_TTL_S
            return ep
