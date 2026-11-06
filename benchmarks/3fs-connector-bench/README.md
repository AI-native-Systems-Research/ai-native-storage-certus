# 3fs-connector-bench

Benchmarks for the **3FS** KV-offload backend: vLLM's in-tree `HF3FSKVConnector`, which writes KV to a [DeepSeek 3FS](https://github.com/deepseek-ai/3fs) cluster through the USRBIO client. This is the backend `deploy/3fs` and `benchmarks/vllm-serve/run-serve-h3fs0-cc131k.sh` serve. The bench is the counterpart of [`certus-connector-bench`](../certus-connector-bench/README.md), [`cputier-connector-bench`](../cputier-connector-bench/README.md) and [`lmcache-connector-bench`](../lmcache-connector-bench/README.md). It has the same modes, report format and CSV row labels, so the backends can be compared side by side without running vLLM.

It drives the connector's own worker-side objects, unchanged:

- `_setup_kv_cache_config` and `_setup_storage_clients`: one `Hf3fsClient` per I/O job over one data file, plus the metadata client
- `AsyncOperationManager`: `submit_save_operation` (gather KV out of the GPU cache, write it with USRBIO, confirm the pages) and `submit_load_operation` (look up the pages, read them, scatter them back), with completions polled as `get_finished` does
- `hf3fs_metadata_server`, the page and key index. Its `batch_key_exists` is the lookup vLLM's scheduler makes in `get_num_new_matched_tokens`. The bench starts a private one on `--metadata-port`, just as vLLM needs one started beside it

No engine, model or HTTP front end is started. The connector object is built without its vLLM-config constructor and given only the attributes its worker path reads. The bench allocates a fake GPU KV cache in the connector's standard `[blocks, heads, block_size, head_dim]` per-layer fp16 view.

## No memory tier

`HF3FSKVConnector` keeps no host-RAM cache: every store goes GPU → 3FS and every load comes 3FS → GPU. So:

- this is a **disk-only baseline**, not a tiered cache, and `--dram` doesn't apply;
- the warm and cold load rows run the same disk path;
- there's no mixed-eviction phase (nothing to evict) and no contention mode;
- a store reports finished only once its write has landed, so no write-through drain is excluded.

## Mock client: code-path tests only

When the 3FS USRBIO client isn't importable (the stock `vllm-openai` image), vLLM quietly swaps in its file-backed **mock** client. The mock uses plain buffered file I/O, so loads can come from the page cache. A mock run is **not** a 3FS measurement. The bench:

- tags every CSV row `hf3fs_client=usrbio` or `mock`, and prints a warning on the mock;
- drops the data file's page cache (`POSIX_FADV_DONTNEED`) before each cold pass on the mock, so its cold reads at least hit the device, and records the device bytes read;
- refuses to run on the mock with `--require-usrbio`. The wrapper passes this flag whenever it runs against a real mount.

The connector-sweep report shows a banner whenever the 3FS arm ran on the mock.

## Differences from the sibling benches

| | certus | cputier | lmcache | 3fs |
|---|---|---|---|---|
| Backend | `certus-server`, `/dev/shm` ring | in-process `TieringOffloadingSpec` | `lmcache server` over ZMQ | in-process connector + metadata server over HTTP |
| Memory tier | `--memory-tier-size` | `--cpu-bytes` | L1 | **none** |
| Disk tier | raw NVMe via SPDK | `fs`: one file per block, `O_DIRECT` | `fs_native`: one file per chunk, `O_DIRECT` | 3FS: pages of one data file, USRBIO over RDMA |
| GPU path | CUDA IPC → server DMA | `swap_blocks` into a pinned mmap | CUDA IPC → server copy | gather/scatter to GPU staging buffers, then USRBIO |
| "Cold" | empty the memory tier | `reset_cache()` | clear L1 | same as warm (no memory tier); the mock drops its page cache |
| Key | one per block | one per block | one per chunk, prefix-chained | one per block (independent keys) |
| Lookup | per key | per key | one round trip per request | one `batch_key_exists` per request |

## Running

Real 3FS needs:

1. The cluster and FUSE mount: `sudo deploy/3fs/run.sh` (see [`deploy/3fs`](../../deploy/3fs/README.md)). It mounts `/mnt/3fs` and creates `/mnt/3fs/vllm`.
2. The vLLM + 3FS image: `benchmarks/vllm-serve/build_h3fs_container.sh 0.30.0` → `localhost/vllm-hf3fs-bench:vllm0.30.0`. Build it from the same 3FS commit as the cluster.

```bash
benchmarks/3fs-connector-bench/run-in-container.sh                     # default phases
benchmarks/3fs-connector-bench/run-in-container.sh --verify            # byte-exact round trip first
benchmarks/3fs-connector-bench/run-in-container.sh --mode all --csv bench-results/3fs.csv --tag 3fs

# no cluster: test the code path on vLLM's mock client (NOT a 3FS measurement)
MOCK=1 benchmarks/3fs-connector-bench/run-in-container.sh --verify
```

Each run uses its own data file (`/mnt/3fs/vllm/bench-<pid>`, or a temp directory under `H3FS_DIR` on the mock) and removes it at exit. The metadata server is private and in memory, so a run always starts empty.

| Flag | Default | Connector setting |
|---|---|---|
| `--file-size` | `64G` | `hf3fs_file_size`: the data file, i.e. the 3FS capacity in use |
| `--numjobs` | `16` | `hf3fs_client_numjobs`: USRBIO clients, which is the I/O parallelism |
| `--max-device-buffer-count` | `128` | `hf3fs_max_device_buffer_count`: GPU staging buffers per direction |
| `--block-bytes` / `--layers` | `2M` / `32` | one key is one GPU block across all layers, which is one 3FS page |

The workload flags (`--mode`, `--bs`, `--num-blocks`, `--min-duration`, `--hit-ratio`, …) match the sibling benches. `--working-set`, `--no-eviction` and `--lookup-order` are accepted for parity but change nothing.

## Jobs in flight (default mode)

The default mode drives the connector as vLLM 0.30 does, and as `certus-connector-bench`'s default mode does: its store, warm and cold passes keep up to `--inflight` jobs of `--job-blocks` keys outstanding. Jobs are submitted without waiting. Each tick makes one non-blocking `get_finished` poll (the connector's `get_finished_operations`) and then sleeps to the next `--step-us` boundary. Stores are submitted one tick after they are prepared, as vLLM defers them past the step.

| Flag | Default | |
|---|---|---|
| `--job-blocks` | 64 | blocks per load job, and per store job unless `--store-job-blocks` is set; at most the connector's staging buffers (`--max-device-buffer-count`, 128), refused above that at argument validation |
| `--inflight` | 4 | jobs outstanding (default mode; prefix-miss and scheduler-step are unchanged and never read `--pipeline-depth`) |
| `--step-us` | 100 | the scheduler tick in µs; 0 = `time.sleep(0)` each tick |
| `--store-job-blocks`, `--store-inflight` | `--job-blocks`, `--inflight` | the store pass's jobs |

`--mode pipelined`, `--pipeline-depth` and `--direction` are gone, as are the one-request-at-a-time default passes. Labels and phase names are kept (`Store Lifecycle (...)`, `Warm Load Lifecycle (...)`, `Cold Load Lifecycle (...)`; `get_finished` is now each job's submit-to-reap time, `reap_call` the one poll per tick, and `load_job` / `store_job` repeat the per-job time). Every CSV row records `job_blocks`, `inflight`, `step_us`, `store_job_blocks` and `store_inflight`. Each job in flight has its own GPU block range.

**What jobs in flight mean for 3FS.** `HF3FSKVConnector`'s `AsyncOperationManager` runs one save thread and one load thread, and each handles one job at a time. Jobs beyond the first queue behind that thread. Its parallelism is within a job: `--numjobs` USRBIO clients over 8-page I/O batches. So more jobs in flight mostly overlap the bench's own host work (lookup, submit) with a running job. A small gain from `--inflight` here is a property of the connector, not of the bench. A load job is staged in GPU buffers from one pool of `max(--max-device-buffer-count, --numjobs x 8)`, 128 by default. vLLM's `start_load_kv` splits a request into batches of that size; this bench refuses `--job-blocks` above it instead, because one job that large fails its buffer allocation.

Wrapper environment: `IMAGE`, `GPU`, `H3FS_MOUNT` (`/mnt/3fs`), `MOCK`, `H3FS_DIR` (mock data, `/raid0/h3fs-mock`), `KEEP_DATA`, `CPUS` (pin with `taskset` inside the container).

In [`connector-sweep`](../connector-sweep/README.md), the 3FS arm runs automatically when `/mnt/3fs` is mounted and is skipped with a note otherwise. `--h3fs-mock` runs it on the mock.
