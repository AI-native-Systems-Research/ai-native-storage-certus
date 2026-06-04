# Certus Performance Matrix

Date: 2026-05-29

Configuration: 4 MiB blocks, 16 objects/batch, 10 iterations per phase, 2 GiB memory-tier pool.

## Aggregate Throughput (GB/s)

| Clients | SSDs | GPUs | Populate | Lookup Hot | Lookup Cold |
|---------|------|------|----------|------------|-------------|
| 1       | 1    | 1    | 9.99     | 16.65      | 2.45        |
| 2       | 1    | 1    | 13.43    | 16.09      | 3.98        |
| 2       | 1    | 2    | 11.01    | 31.99      | 5.75        |
| 8       | 1    | 1    | 13.81    | 20.51      | 8.20        |
| 8       | 1    | 2    | 12.37    | 24.43      | 9.45        |
| 1       | 2    | 1    | 11.02    | 16.50      | 6.82        |
| 2       | 2    | 1    | 13.54    | 17.12      | 9.13        |
| 2       | 2    | 2    | 10.48    | 17.91      | 10.47       |
| 8       | 2    | 1    | 14.71    | 18.63      | 10.84       |
| 8       | 2    | 2    | 14.81    | 9.43       | 11.52       |
| 1       | 4    | 1    | 10.49    | 17.35      | 7.05        |
| 2       | 4    | 1    | 12.95    | 14.86      | 8.53        |
| 2       | 4    | 2    | 9.81     | 19.86      | 9.12        |
| 8       | 4    | 1    | 14.68    | 20.12      | 10.92       |
| 8       | 4    | 2    | 13.30    | 17.22      | 11.32       |

## Notes

- 1-client/2-GPU configurations omitted (round-robin leaves one GPU idle)
- SSDs: Intel/Samsung NVMe on PCIe slots 0000:61-64:00.0
- GPUs: 2x NVIDIA (CUDA devices 0, 1)
- Populate saturates at ~14-15 GB/s with 8 clients (gRPC/CPU bound)
- Hot lookups peak at ~32 GB/s (2 clients, 2 GPUs, memory-tier)
- Cold lookups scale with both SSD count and client concurrency (2.45 -> 11.52 GB/s)
