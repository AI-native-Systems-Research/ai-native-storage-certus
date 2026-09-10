# Run Certus-shmq with Prometheus driver (manually start client and server)

## Server

```bash
numactl --cpunodebind=0 --membind=0 target/release/certus-server   --device-pci 0000:61:00.0 --device-pci 0000:62:00.0 --device-pci 0000:63:00.0   --shm-path /dev/shm/certus-shmq   --memory-tier-size 13G   --memory-tier-eviction-threshold 0.9   --store-backpressure-ms 5000   --channels 32   --poller-base-cpu 2   --shmq-poller-cpu 6   --format
```

## Client

Limit to 10 conversations. Use real recorded time-stamps from otel replay data (e.g., from inference-perf).

TIME_SCALE parameter:
┌─────────────┬─────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│    Value    │                                                   Effect                                                    │
├─────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ 1.0         │ Real recorded timing — reproduces the workload's actual pacing. Runs long: the deepest convs carry          │
│ (default)   │ ~2000–2800 s of recorded delay, so wall-clock ≈ the longest conversation.                                   │
├─────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ 0           │ Saturating — skip all waits, fire turns as fast as the engine takes them. Good for smoke tests and for      │
│             │ stressing the tier hard.                                                                                    │
├─────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ 0.1         │ 10× faster — real timing compressed (gaps scaled to a tenth).                                               │
├─────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ > 1         │ Slower than real (stretches the gaps).                                                                      │
└─────────────┴─────────────────────────────────────────────────────────────────────────────────────────────────────────────┘

ACTIVE_SESSIONS and MAX_NUM_SEQ:

┌───────────────────────────────────────┬───────────────────────────────────────────────────────────────────────────────────┐
│               Scenario                │                                      Effect                                       │
├───────────────────────────────────────┼───────────────────────────────────────────────────────────────────────────────────┤
│ ACTIVE_SESSIONS=0, MAX_NUM_SEQS=64,   │ Driver floods vLLM with up to 1000 requests; vLLM runs 64 at a time, the other    │
│ 1000 convs                            │ ~936 wait in its queue. 64 is the real bottleneck.                                │
├───────────────────────────────────────┼───────────────────────────────────────────────────────────────────────────────────┤
│ ACTIVE_SESSIONS=32, MAX_NUM_SEQS=64   │ Driver only ever offers ≤32 requests; vLLM's 64 cap is never reached.             │
│                                       │ ACTIVE_SESSIONS is the bottleneck — batch tops out at 32.                         │
├───────────────────────────────────────┼───────────────────────────────────────────────────────────────────────────────────┤
│ ACTIVE_SESSIONS=0, MAX_NUM_SEQS=64,   │ All convs alive, but most are asleep in their recorded think-gap, so far fewer    │
│ TIME_SCALE=1.0                        │ than 64 are submitting → batch sits well below 64.                                │
└───────────────────────────────────────┴───────────────────────────────────────────────────────────────────────────────────┘

```bash
NUM_CONVS=10 OTEL_HOST=/mnt/certus1/inference-perf-syn-data/otel_1k ACTIVE_SESSIONS=32 TIME_SCALE=1.0 ./run-docker-otel-shmq-prom.sh
```
