# deploy/3fs — single-node DeepSeek 3FS in one privileged container

This directory stands up a self-contained [3FS](https://github.com/deepseek-ai/3fs) cluster on this host. The cluster is FoundationDB plus mgmtd, meta, storage and the FUSE client. Its mount is exported to the host so that vLLM's `HF3FSKVConnector` can use 3FS as its KV-offload tier. That makes 3FS directly comparable with Certus, through `benchmarks/vllm-serve/run-serve-h3fs0-cc131k.sh` and `run-guidellm-cc131k.sh`.

```
 host                                       ┌─ certus-3fs (rootful, --privileged,
                                            │   --network host, --ipc host)
 /cache/3fs  (nvme0n1, XFS)  ── /data ──────┤  fdbserver        127.0.0.1:4500
                                            │  mgmtd_main       :8100 (RDMA) :9100 (TCP)
                                            │  meta_main        :8101        :9101
                                            │  storage_main     :8102        :9102
 /mnt/3fs  ◀── rshared ── fuse.hf3fs ───────┤  hf3fs_fuse_main  → /mnt/3fs
     │                                      └─ RDMA over mlx5_0 / ens6f0np0 (RoCE)
     │  bind + --ipc host  (USRBIO buffers go through /dev/shm)
     ▼
 vllm-hf3fs-bench (rootless)  vllm serve + HF3FSKVConnector + metadata server :18000
```

## Files

| File | Runs as | Purpose |
|---|---|---|
| `Containerfile` | — | Ubuntu 22.04 build of the five 3FS binaries, plus the FDB server and the bootstrap scripts |
| `build.sh` | you | Builds `localhost/certus-3fs-cluster`. It pins the 3FS commit to the client wheel's in `vllm-hf3fs-bench` |
| `entrypoint.sh` | (container) | Starts the services in order. On first boot it also runs init-cluster and creates the user, targets and chain table |
| `render_configs.py` | (container) | Fills in the upstream config templates. It fails if an expected key is missing |
| `run.sh` | **root** | Starts the container and waits for `/mnt/3fs` |
| `smoke.sh` | you | Exercises the FUSE path and the real USRBIO path (the connector's own calls), and checks the data |
| `admin.sh` | **root** | `admin_cli` passthrough (`list-nodes`, `list-chains`, …) |
| `teardown.sh` | **root** | Stops the container and unmounts. `--wipe` also deletes the cluster data |

## Use

```bash
cd ~/certus/deploy/3fs
./build.sh                       # ~30-60 min the first time (3FS + folly + rocksdb)
sudo ./run.sh                    # first boot initializes the cluster in /cache/3fs
./smoke.sh                       # FUSE + USRBIO round trip
sudo ./admin.sh                  # nodes + chains

# then, from benchmarks/vllm-serve:
HF3FS_MOUNT_HOST=/mnt/3fs ./run-serve-h3fs0-cc131k.sh
./run-guidellm-cc131k.sh
```

Restarting with `sudo ./teardown.sh && sudo ./run.sh` brings the **same** cluster back. FDB, targets and the token persist in `/cache/3fs`. To start fresh, run `sudo ./teardown.sh --wipe`.

### Building before the client image exists

By default `build.sh` starts the client image (`localhost/vllm-hf3fs-bench:vllm0.30.0`, from podman's default store) to read the 3FS commit. If that image hasn't been built on this host, podman tries to pull it from a registry at `localhost` and fails:

```
Error: unable to copy from source docker://localhost/vllm-hf3fs-bench:vllm0.30.0: ... connection refused
error: cannot read hf3fs_py_usrbio version from localhost/vllm-hf3fs-bench:vllm0.30.0; set HF3FS_REF
```

Set the commit yourself to skip the lookup. For the vllm0.30.0 client it is `22fca04`:

```bash
HF3FS_REF=22fca04 ./build.sh
STORE_DIR=/mnt/certus1/podman HF3FS_REF=22fca04 ./build.sh    # with a custom build store
```

You still need the client image for `smoke.sh` and `run-serve-h3fs0-cc131k.sh`. Build it from the **same** commit so the server and client match: `cd benchmarks/vllm-serve && HF3FS_REF=22fca04 ./build_h3fs_container.sh 0.30.0`.

## Where podman stores images (`STORE_DIR`)

The root filesystem here is 70 GB and nearly full. By default, root's podman store (`/var/lib/containers`) and podman's staging directory (`/var/tmp`) both live on it, so `run.sh` can't copy the 3.2 GB image in. Point the store at a folder on a bigger filesystem. The rootless (build) store and the rootful (run) store must be **different** folders:

```bash
STORE_DIR=/home/dwaddington/podman-3fs ./build.sh                   # optional: build store too
sudo BUILD_STORE_DIR=/home/dwaddington/podman-3fs \
     STORE_DIR=/home/podman-3fs-root ./run.sh            # root's store + tmp here
sudo ./admin.sh; sudo ./teardown.sh                              # reuse run.sh's STORE_DIR (saved)
```

`STORE_DIR` maps to `podman --root $STORE_DIR/storage --runroot $STORE_DIR/run` with `TMPDIR=$STORE_DIR/tmp`. Leave `BUILD_STORE_DIR` unset if `build.sh` used the default rootless store. The image is copied across once; later runs reuse it.

## Knobs (`run.sh` environment)

| Var | Default | Notes |
|---|---|---|
| `DATA_DIR` | `/cache/3fs` | FDB data, storage targets, logs, token. Keep it on its own NVMe so I/O isn't shared with Certus |
| `MOUNT` | `/mnt/3fs` | Host mountpoint |
| `NET_IFACE` / `IB_DEVICE` | `ens6f0np0` / `mlx5_0` | The *active* RoCE port. mlx5_1 is DOWN on this host |
| `NUM_CHAINS` | `16` | Number of targets/chains on the single disk. Also the file stripe (≤16). Fixed at init |
| `MGMTD_PORT` | `8100` | Base port. Uses 8100–8102 and 9100–9102, which keeps upstream's 8000 free for vLLM |

Parameters that are fixed at init are written to `state/params`. If they change later, the container refuses to start rather than silently running a mismatched cluster.

## Design notes and caveats for the comparison

- **RDMA is required; there is no TCP mode.** The storage service moves file data only by RDMA, reading from or writing to the client's registered buffer. Over TCP, metadata calls work but every data write or read fails with `update no RDMA socket` / `batch read no RDMA socket`, and FUSE returns `Input/output error`. On this host that means RoCE v2 over `mlx5_0`.
- **Replication factor is 1.** There is one storage node and one disk, so there's no CRAQ replication traffic. This is 3FS with its main durability cost removed. That's the fairest match to a single-node Certus, but say so in any results.
- **Chain table is generated by hand.** Upstream's `data_placement` tool refuses a single node (see the FAQ in `deploy/README.md`). `entrypoint.sh` writes the equivalent `create-target`, chains and chain-table files, using the same ID scheme as `gen_chain_table.py`.
- **No monitor or ClickHouse.** The `monitor_collector` reporters are removed from the configs, and logs go to `/cache/3fs/log/`.
- **Server and client versions must match.** `build.sh` reads `hf3fs_py_usrbio <ver>+<sha>` from the client image and builds the server from that `<sha>` with the same `SHUFFLE_METHOD`. If you rebuild `vllm-hf3fs-bench` from a different `HF3FS_REF`, rebuild this image too.
- **`--ipc host` is needed on both sides.** USRBIO's `make_iovec` symlinks a `/dev/shm` segment into `<mount>/3fs-virt/iovs/`, and `hf3fs_fuse_main` maps it by path.
- **The metadata server is not started by vLLM.** In vLLM 0.30 the connector always uses `http://localhost:18000`, so `run-serve-h3fs0-cc131k.sh` starts `hf3fs_metadata_server` inside the vLLM container first.
- **Storage targets are a directory, not a raw disk.** `/cache/3fs/storage` sits on the existing XFS filesystem (`allow_disk_without_uuid = true`, because a bind mount has no `/dev/disk/by-uuid` entry). Nothing is reformatted.
