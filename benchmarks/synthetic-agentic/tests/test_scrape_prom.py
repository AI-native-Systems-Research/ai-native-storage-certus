"""scrape_prom.py over several sources, against fake /metrics servers."""

import http.server
import os
import signal
import subprocess
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
SCRAPER = os.path.join(HERE, "..", "scrape_prom.py")
sys.path.insert(0, os.path.join(HERE, ".."))

import scrape_prom  # noqa: E402


class Fake:
    """A /metrics server whose counters the test moves; ``down`` fails scrapes."""

    def __init__(self, text):
        self.text = text
        self.down = False
        fake = self

        class H(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                if fake.down:
                    self.send_error(503)
                    return
                body = fake.text.encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *a):
                pass

        self.srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()
        self.url = f"http://127.0.0.1:{self.srv.server_address[1]}/metrics"


def vllm(hits):
    return f'vllm:prefix_cache_hits_total{{engine="0"}} {hits}\n'


def certus(fetched):
    return f"certus_lookup_hits_fetched_total {fetched}\ncertus_memory_tier_used_bytes 5\n"


def rounds(path):
    out = []
    for line in open(path):
        if line.startswith("[prom] round"):
            kv = dict(t.split("=", 1) for t in line.split(": ", 1)[1].split())
            out.append({k: float(v) for k, v in kv.items()})
    return out


def run(tmp_path, urls, steps, interval=0.4):
    """Start the scraper, apply ``steps`` (callables) one per interval, stop it."""
    out = tmp_path / "run.prom.log"
    args = [sys.executable, SCRAPER, "--interval", str(interval), "--out", str(out)]
    for u in urls:
        args += ["--url", u]
    p = subprocess.Popen(args, stderr=subprocess.PIPE, text=True)
    time.sleep(interval * 0.5 + 0.5)  # past the baseline
    for step in steps:
        step()
        time.sleep(interval)
    p.send_signal(signal.SIGTERM)
    p.wait(timeout=10)
    return rounds(out)


def test_sources_sum_and_split_by_label(tmp_path):
    a, b, c = Fake(vllm(10)), Fake(vllm(100)), Fake(certus(0))

    def move():
        a.text, b.text, c.text = vllm(13), vllm(105), certus(7)

    rs = run(tmp_path, [f"prefill={a.url}", f"decode={b.url}", f"certus-n2={c.url}"], [move])
    tot = lambda k: sum(r.get(k, 0) for r in rs)  # noqa: E731
    assert tot("prefix_cache_hits") == 8, rs
    assert tot("prefill.prefix_cache_hits") == 3
    assert tot("decode.prefix_cache_hits") == 5
    assert tot("certus_lookup_hits_fetched") == 7
    assert tot("certus-n2.certus_lookup_hits_fetched") == 7
    # Gauges are not counters: never emitted.
    assert not any("memory_tier_used" in k for r in rs for k in r)


def test_a_missed_scrape_lands_whole_on_the_next(tmp_path):
    a, b = Fake(vllm(0)), Fake(vllm(0))

    def b_down_while_moving():
        b.down = True
        a.text, b.text = vllm(1), vllm(50)

    def b_back():
        b.down = False

    rs = run(tmp_path, [a.url, b.url], [b_down_while_moving, b_back])
    assert sum(r.get("prefix_cache_hits", 0) for r in rs) == 51, rs
    assert all(v >= 0 for r in rs for v in r.values())


def test_one_source_keeps_the_old_line(tmp_path):
    a = Fake(vllm(0))

    def move():
        a.text = vllm(4)

    rs = run(tmp_path, [f"x={a.url}"], [move])
    assert sum(r.get("prefix_cache_hits", 0) for r in rs) == 4
    assert not any("." in k for r in rs for k in r), "no per-label keys for one source"


def test_repeated_roles_are_numbered():
    got = scrape_prom._unique_labels([("prefill", "a"), ("decode", "b"), ("decode", "c")])
    assert [label for label, _ in got] == ["prefill", "decode0", "decode1"]
