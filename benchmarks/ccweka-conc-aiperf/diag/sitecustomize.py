"""certus debug hook: dump vLLM scheduler + KV-offload connector state on SIGUSR2.

Mounted into the cputier container and put first on PYTHONPATH, so it loads in
every Python process of the container. It only does anything in a process that
holds a vLLM Scheduler (the EngineCore; with TP=1 that process also holds the
worker-side connector and the GPU copy handlers). It never mutates state.

Purpose: classify why waiting requests are "deferred" during the cputier stall:
  A. WAITING_FOR_REMOTE_KVS with no finished_recving report (lost load completion)
  B. connector returned None: in-flight transfers / backend lookup pending /
     hit chunks in _chunks_being_loaded
and, for B-via-_chunks_being_loaded, whether the blocking keys belong to a live
load job or are orphaned (leaked).

Output: $CERTUS_DEBUG_OUT/sched-dump-<pid>-<HHMMSS>.txt
Trigger: kill -USR2 <EngineCore pid>
"""

import os
import sys

# Chain to the image's own sitecustomize, which this file shadows on PYTHONPATH.
_ORIG = "/usr/lib/python3.12/sitecustomize.py"
try:
    if os.path.exists(_ORIG):
        with open(_ORIG) as _f:
            exec(compile(_f.read(), _ORIG, "exec"), {"__name__": "_orig_sitecustomize"})
except Exception as _e:  # never break interpreter startup
    print(f"[certus-debug] original sitecustomize failed: {_e!r}", file=sys.stderr)

import collections
import gc
import signal
import threading
import time
import traceback

_OUT = os.environ.get("CERTUS_DEBUG_OUT", "/tmp")
_MAX_ROWS = int(os.environ.get("CERTUS_DEBUG_MAX_ROWS", "200"))


def _instances(cls):
    out = []
    for o in gc.get_objects():
        try:
            if isinstance(o, cls):
                out.append(o)
        except Exception:
            pass
    return out


def _ln(x):
    try:
        return len(x)
    except Exception:
        return "?"


def _section(f, title, fn):
    f.write(f"\n===== {title} =====\n")
    try:
        fn()
    except Exception:
        f.write("SECTION FAILED:\n" + traceback.format_exc())


def _req_keys(rs):
    keys = []
    for gs in getattr(rs, "group_states", ()) or ():
        keys.extend(getattr(gs, "offload_keys", ()) or ())
    return keys


def _schedulers_from_frames():
    """Walk every thread's live frames for a `self` that owns a `.scheduler` with
    waiting/running queues (the EngineCore). Independent of gc tracking/freezing."""
    seen, out = set(), []
    for fr in sys._current_frames().values():
        while fr is not None:
            obj = fr.f_locals.get("self")
            sch = getattr(obj, "scheduler", None) if obj is not None else None
            if sch is not None and hasattr(sch, "waiting") and hasattr(sch, "running") and id(sch) not in seen:
                seen.add(id(sch))
                out.append(sch)
            fr = fr.f_back
    return out


def _find_schedulers():
    """Return (schedulers, how). Frame walk first (precise); then isinstance against
    vllm's Scheduler; then a *Scheduler name match, so a renamed class still dumps."""
    found = _schedulers_from_frames()
    if found:
        return found, "frame walk (EngineCore.self.scheduler)"
    smod = sys.modules.get("vllm.v1.core.sched.scheduler")
    if smod is not None and hasattr(smod, "Scheduler"):
        found = _instances(smod.Scheduler)
        if found:
            return found, "isinstance(vllm.v1.core.sched.scheduler.Scheduler)"
    found = []
    for o in gc.get_objects():
        try:
            if any(c.__name__.endswith("Scheduler") for c in type(o).__mro__) and hasattr(o, "waiting") and hasattr(o, "running"):
                found.append(o)
        except Exception:
            pass
    return found, f"name-match fallback (scheduler module loaded={smod is not None})"


def _dump(f):
    scheds, how = _find_schedulers()
    _breadcrumb(f"found {len(scheds)} scheduler(s) via {how}: {[type(s).__module__ + '.' + type(s).__name__ for s in scheds]}")
    if not scheds:
        return False
    now_wall, now_mono = time.time(), time.monotonic()
    f.write(f"certus debug dump pid={os.getpid()} wall={now_wall:.3f} mono={now_mono:.3f}\n")

    for s in scheds:
        waiting = list(getattr(s, "waiting", ()) or ())
        skipped = list(getattr(s, "skipped_waiting", ()) or ())
        running = list(getattr(s, "running", ()) or ())
        frkv = set(getattr(s, "finished_recving_kv_req_ids", ()) or ())
        cs = getattr(getattr(s, "connector", None), "connector_scheduler", None)
        being = getattr(cs, "_chunks_being_loaded", None) if cs is not None else None
        being = set(being) if being is not None else None
        jobs = dict(getattr(cs, "_jobs", {}) or {}) if cs is not None else {}
        req_status = dict(getattr(cs, "_req_status", {}) or {}) if cs is not None else {}

        def sched_summary():
            f.write(f"running={len(running)} waiting={len(waiting)} skipped_waiting={len(skipped)}\n")
            f.write(f"finished_recving_kv_req_ids={len(frkv)} {sorted(frkv)[:20]}\n")
            hist = collections.Counter(getattr(r.status, "name", str(r.status)) for r in waiting + skipped)
            f.write(f"waiting status histogram: {dict(hist)}\n")
            f.write(f"connector_scheduler={type(cs).__name__ if cs is not None else None}\n")
            if cs is not None:
                f.write(
                    f"_chunks_being_loaded={_ln(being) if being is not None else None} "
                    f"_jobs={len(jobs)} _req_status={len(req_status)} "
                    f"_job_counter={getattr(cs, '_job_counter', '?')} "
                    f"_stale_job_threshold={getattr(cs, '_stale_job_threshold', '?')} "
                    f"_current_batch_load_jobs={_ln(getattr(cs, '_current_batch_load_jobs', ()))}\n"
                )

        def per_request():
            f.write(
                "req_id | status | computed/prompt/total | in finished_recving | "
                "transfer_jobs | deferred_lookup_age_s | n_keys | n_keys_in_being_loaded | "
                "partial_tail | finished_signaled\n"
            )
            for r in (waiting + skipped)[:_MAX_ROWS]:
                rid = r.request_id
                rs = req_status.get(rid)
                keys = _req_keys(rs) if rs is not None else []
                nb = sum(1 for k in keys if being and k in being)
                dls = getattr(rs, "deferred_lookup_start_time", None) if rs is not None else None
                age = f"{now_mono - dls:.1f}" if dls else "-"
                f.write(
                    f"{rid} | {getattr(r.status, 'name', r.status)} | "
                    f"{r.num_computed_tokens}/{getattr(r, 'num_prompt_tokens', '?')}/{r.num_tokens} | "
                    f"{rid in frkv} | "
                    f"{sorted(rs.transfer_jobs) if rs is not None else 'NO_REQ_STATUS'} | {age} | "
                    f"{len(keys)} | {nb} | "
                    f"{getattr(rs, 'partial_tail_boundary', '-') if rs is not None else '-'} | "
                    f"{getattr(rs, 'finished_signaled', '-') if rs is not None else '-'}\n"
                )

        def job_table():
            reqs = getattr(s, "requests", {}) or {}
            load_keys = set()
            f.write("job_id | req_id | req_status_in_scheduler | is_store | pending_count | n_keys | n_keys_in_being_loaded\n")
            for jid, js in sorted(jobs.items())[:_MAX_ROWS]:
                keys = set(getattr(js, "keys", ()) or ())
                if not js.is_store:
                    load_keys |= keys
                r = reqs.get(js.req_id)
                rst = getattr(getattr(r, "status", None), "name", "GONE") if r is not None else "GONE"
                nb = len(keys & being) if being else 0
                f.write(f"{jid} | {js.req_id} | {rst} | {js.is_store} | {js.pending_count} | {len(keys)} | {nb}\n")
            if being is not None:
                orphans = being - load_keys
                f.write(
                    f"\n_chunks_being_loaded total={len(being)} covered_by_live_load_jobs={len(being & load_keys)} "
                    f"ORPHANED={len(orphans)}\n"
                )
                # which waiting requests are blocked by orphaned keys
                blocked = []
                for r in waiting + skipped:
                    rs = req_status.get(r.request_id)
                    if rs is not None and any(k in orphans for k in _req_keys(rs)):
                        blocked.append(r.request_id)
                f.write(f"waiting requests touching ORPHANED keys: {len(blocked)} {blocked[:20]}\n")

        def tier_manager():
            m = getattr(cs, "manager", None)
            f.write(f"manager={type(m).__name__ if m is not None else None}\n")
            if m is None:
                return
            mj = dict(getattr(m, "_jobs", {}) or {})
            f.write(f"manager._jobs={len(mj)}\n")
            for jid, jm in sorted(mj.items())[:_MAX_ROWS]:
                tj = getattr(jm, "transfer_job", None)
                rc = getattr(tj, "req_context", None)
                f.write(
                    f"  mjob {jid} tier={getattr(jm, 'tier_idx', '?')} "
                    f"promotion={getattr(tj, 'is_promotion', '?')} keys={_ln(getattr(tj, 'keys', ()))} "
                    f"req={getattr(rc, 'req_id', '?')}\n"
                )
            pls = getattr(m, "_pending_load_submissions", {}) or {}
            f.write(f"_pending_load_submissions tiers={len(pls)} sizes={ {k: len(v) for k, v in pls.items()} }\n")
            f.write(f"_req_state={_ln(getattr(m, '_req_state', ()))} _processed_jobs_this_step={getattr(m, '_processed_jobs_this_step', '?')}\n")

        _section(f, "scheduler summary", sched_summary)
        _section(f, "waiting requests", per_request)
        _section(f, "connector jobs", job_table)
        _section(f, "tiering manager", tier_manager)

    def async_lookups():
        amod = sys.modules.get("vllm.v1.kv_offload.tiering.async_lookup")
        if amod is None:
            f.write("async_lookup module not loaded\n")
            return
        for al in _instances(amod.AsyncLookupManager):
            st = dict(getattr(al, "_lookup_state", {}) or {})
            ph = collections.Counter(getattr(getattr(v, "phase", None), "name", "?") for v in st.values())
            res = collections.Counter(str(getattr(v, "result", "?")) for v in st.values())
            lq = getattr(al, "_lookup_queue", None)
            pr = getattr(al, "_pending_results", None)
            f.write(
                f"{type(al).__name__} tier={getattr(al, '_tier_type', '?')} states={len(st)} phases={dict(ph)} "
                f"results={dict(res)} batch={_ln(getattr(al, '_lookup_batch', ()))} "
                f"lookup_queue={lq.qsize() if hasattr(lq, 'qsize') else '?'} "
                f"pending_results={pr.qsize() if hasattr(pr, 'qsize') else '?'} "
                f"need_to_drain={getattr(al, '_need_to_drain', '?')} req_keys={_ln(getattr(al, '_req_keys', ()))}\n"
            )
            # oldest IN_FLIGHT / PENDING generations (a stuck lookup shows as a low generation)
            stuck = sorted(
                (v.generation, v.phase.name, len(v.request_ids))
                for v in st.values()
                if getattr(v, "result", None) is None
            )[:10]
            f.write(f"  unresolved (generation, phase, n_requests): {stuck}\n")
            f.write(f"  next_generation={getattr(al, '_next_generation', '?')}\n")

    def worker_side():
        wmod = sys.modules.get("vllm.distributed.kv_transfer.kv_connector.v1.offloading.worker")
        if wmod is not None:
            for w in _instances(wmod.OffloadingConnectorWorker):
                lj = dict(getattr(w, "_load_jobs", {}) or {})
                f.write(
                    f"OffloadingConnectorWorker _load_jobs={len(lj)} {sorted(lj.items())[:20]} "
                    f"_unsubmitted_store_jobs={_ln(getattr(w, '_unsubmitted_store_jobs', ()))}\n"
                )
        gmod = sys.modules.get("vllm.v1.kv_offload.cpu.gpu_worker")
        if gmod is not None:
            for h in _instances(gmod.SingleDirectionOffloadingHandler):
                tr = list(getattr(h, "_transfers", ()) or ())
                head = []
                for t in tr[:5]:
                    try:
                        q = t.end_event.query()
                    except Exception as e:
                        q = f"query-err {e!r}"
                    head.append((t.job_id, q))
                f.write(
                    f"GPU handler gpu_to_cpu={getattr(h, 'gpu_to_cpu', '?')} transfers={len(tr)} "
                    f"head(job_id, end_event_done)={head} events={_ln(getattr(h, '_transfer_events', ()))}\n"
                )
        fmod = sys.modules.get("vllm.v1.kv_offload.tiering.fs.thread_pool")
        if fmod is not None:
            for p in _instances(fmod.DualQueueThreadPool):
                attrs = {
                    k: _ln(v)
                    for k, v in vars(p).items()
                    if isinstance(v, (dict, set, list, collections.deque))
                }
                f.write(f"DualQueueThreadPool containers={attrs}\n")

    def threads():
        for t in threading.enumerate():
            if not t.name.startswith(("vllm_kv_py_fs_", "Thread-", "ThreadPoolExecutor")):
                f.write(f"thread {t.name} alive={t.is_alive()} daemon={t.daemon}\n")
        f.write(f"total threads={threading.active_count()}\n")

    _section(f, "async lookups", async_lookups)
    _section(f, "worker side (connector + GPU handlers + fs pool)", worker_side)
    _section(f, "threads", threads)
    return True


def _breadcrumb(msg):
    """Append one line to hook.log so every load and every signal leaves a trace."""
    try:
        with open(os.path.join(_OUT, "hook.log"), "a") as f:
            f.write(f"{time.strftime('%H:%M:%S')} pid={os.getpid()} {msg}\n")
    except Exception:
        pass


def _on_usr2(signum, frame):
    t0 = time.monotonic()
    path = os.path.join(_OUT, f"sched-dump-{os.getpid()}-{time.strftime('%H%M%S')}.txt")
    # vLLM gc.freeze()s its heap after startup; frozen objects are invisible to
    # gc.get_objects(). Unfreeze for the scan and refreeze after (no collection runs).
    frozen = gc.get_freeze_count()
    _breadcrumb(f"SIGUSR2 received (gc frozen objects={frozen})")
    if frozen:
        gc.unfreeze()
    try:
        with open(path + ".tmp", "w") as f:
            found = _dump(f)
        if found:
            os.replace(path + ".tmp", path)
            _breadcrumb(f"dump written {os.path.basename(path)} in {time.monotonic() - t0:.1f}s")
        else:
            os.unlink(path + ".tmp")
            _breadcrumb(f"no scheduler in this process; nothing written ({time.monotonic() - t0:.1f}s)")
    except Exception:
        _breadcrumb("dump FAILED, see .err")
        try:
            with open(path + ".err", "w") as f:
                f.write(traceback.format_exc())
        except Exception:
            pass
    finally:
        if frozen:
            gc.freeze()


try:
    signal.signal(signal.SIGUSR2, _on_usr2)
    _breadcrumb(f"hook loaded, SIGUSR2 installed, argv={sys.argv[:3]}")
except Exception as _e:  # not the main thread / unsupported: stay inert
    _breadcrumb(f"SIGUSR2 NOT installed: {_e!r}")
    print(f"[certus-debug] SIGUSR2 hook not installed: {_e!r}", file=sys.stderr)
