#!/usr/bin/env python3
"""Render single-node 3FS configs from the upstream templates in this image.

Upstream ships every *.toml with empty addresses/paths (configs/ in deepseek-ai/3fs).
This applies a small, explicit set of line-level edits — section-aware, so the same
key in two sections is never confused — and FAILS if a key it expects is missing,
which is how upstream template drift shows up instead of a silently wrong config.

Inputs come from the environment (set by entrypoint.sh):
  CLUSTER_ID  MGMTD_ADDR  FDB_CLUSTER_FILE  TOKEN_FILE  MOUNTPOINT  STORAGE_PATH
  NET_IFACE   IB_DEVICE   NETWORK (RDMA|TCP)  PORTS (json: per-service port remaps)
"""
import json
import os
import re
import sys

SRC, DST = sys.argv[1], sys.argv[2]
E = os.environ
TCP = E["NETWORK"].upper() == "TCP"
PORTS = json.loads(E["PORTS"])


def q(s):
    return "'" + s + "'"


def qlist(items):
    return "[ " + ", ".join(q(i) for i in items) + " ]"


HEADER = re.compile(r"^\s*\[\[?\s*([^\]]+?)\s*\]\]?\s*$")


def render(name, sets=(), setall=(), remap=None, drop_prefix=None):
    lines = open(os.path.join(SRC, name)).read().splitlines()
    out, section, dropping = [], "", False
    hit = {k: 0 for k in sets}
    for line in lines:
        m = HEADER.match(line)
        if m:
            section = m.group(1)
            dropping = bool(drop_prefix) and section.startswith(drop_prefix)
        if dropping:
            continue
        km = re.match(r"^(\s*)([A-Za-z0-9_]+)\s*=", line)
        if km and not m:
            key = km.group(2)
            if (section, key) in sets:
                line = f"{km.group(1)}{key} = {sets[(section, key)]}"
                hit[(section, key)] += 1
            elif key in setall:
                line = f"{km.group(1)}{key} = {setall[key]}"
            elif remap and key == "listen_port":
                old = int(line.split("=", 1)[1])
                if old in remap:
                    line = f"{km.group(1)}listen_port = {remap[old]}"
        out.append(line)
    missing = [k for k, n in hit.items() if n == 0]
    if missing:
        sys.exit(f"render_configs: {name}: expected keys not found (template drift?): {missing}")
    with open(os.path.join(DST, name), "w") as f:
        f.write("\n".join(out) + "\n")


mgmtd = qlist([E["MGMTD_ADDR"]])
fdb = q(E["FDB_CLUSTER_FILE"])
cid = q(E["CLUSTER_ID"])

# Keys that apply everywhere they appear in a file.
common = {"device_filter": qlist([E["IB_DEVICE"]]), "filter_list": qlist([E["NET_IFACE"]])}
# TCP mode = clients force TCP; servers stay RDMA-typed. The storage client only sends
# data I/O to endpoints whose type is RDMA (StorageClientImpl.cc callMessengerMethod),
# so TCP-typed server groups make every write fail with NoRDMAInterface. RDMA-typed
# listeners also accept TCP, and force_use_tcp rewrites the address to TCP
# (net/Client.h), which is how 3FS itself runs over TCP.
if TCP:
    common.update(force_use_tcp="true")
REPORTERS = "common.monitor.reporters"  # no monitor_collector/ClickHouse in this setup


def ports(svc):
    return {int(k): v for k, v in PORTS[svc].items()}


render("admin_cli.toml",
       sets={("", "cluster_id"): cid, ("fdb", "clusterFile"): fdb,
             ("mgmtd_client", "mgmtd_server_addresses"): mgmtd},
       setall=common)

render("mgmtd_main_launcher.toml",
       sets={("", "cluster_id"): cid, ("fdb", "clusterFile"): fdb,
             ("kv_engine.fdb", "clusterFile"): fdb},
       setall=common)
render("mgmtd_main_app.toml", sets={("", "node_id"): "1"})
render("mgmtd_main.toml", setall=common, remap=ports("mgmtd"), drop_prefix=REPORTERS)

render("meta_main_launcher.toml",
       sets={("", "cluster_id"): cid, ("mgmtd_client", "mgmtd_server_addresses"): mgmtd},
       setall=common)
render("meta_main_app.toml", sets={("", "node_id"): "100"})
render("meta_main.toml",
       sets={("server.fdb", "clusterFile"): fdb, ("server.kv_engine.fdb", "clusterFile"): fdb,
             ("server.mgmtd_client", "mgmtd_server_addresses"): mgmtd},
       setall=common, remap=ports("meta"), drop_prefix=REPORTERS)

render("storage_main_launcher.toml",
       sets={("", "cluster_id"): cid, ("mgmtd_client", "mgmtd_server_addresses"): mgmtd},
       setall=common)
render("storage_main_app.toml", sets={("", "node_id"): "10001"})
render("storage_main.toml",
       sets={("server.mgmtd", "mgmtd_server_addresses"): mgmtd,
             ("server.targets", "target_paths"): qlist([E["STORAGE_PATH"]]),
             # target dir is a bind mount inside a container — no /dev/disk/by-uuid
             ("server.targets", "allow_disk_without_uuid"): "true"},
       setall=common, remap=ports("storage"), drop_prefix=REPORTERS)

render("hf3fs_fuse_main_launcher.toml",
       sets={("", "cluster_id"): cid, ("", "mountpoint"): q(E["MOUNTPOINT"]),
             ("", "token_file"): q(E["TOKEN_FILE"]),
             ("mgmtd_client", "mgmtd_server_addresses"): mgmtd},
       setall=common)
render("hf3fs_fuse_main.toml",
       sets={("mgmtd", "mgmtd_server_addresses"): mgmtd},
       setall=common, drop_prefix=REPORTERS)

print(f"render_configs: wrote configs to {DST} (network={'TCP' if TCP else 'RDMA'})")
