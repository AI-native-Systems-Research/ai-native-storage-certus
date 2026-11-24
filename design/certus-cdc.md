# Certus Constitutional Design Constraints (CDC)

These are design requirement that are explicitly defined by the human-developer(s) and should hold or be exploited as fit.

1. Data should not be striped across multiple SSDs so that in the event of SSD failure, only the subset of data stored on the corresponding drive is lost.  Data and its associated meta-data should both reside on the same SSD.
2. It is acceptable for data (cache blocks) to be lost in the event of software or hardware failure. Resources allocated for lost data can remain un-accounted for until the next explicit storage format.
3. Partial data loss or corrupt data must be avoided.  If cached data is potentially corrupt, it should be evicted accordingly.
4. Erasure coding is considered too expensive to protect against SSD failure - read and write throughput and latency are primary objectives.
5. The vLLM or LLM-d code should not be explicitly modified; only available 'hooks' or plugin architectures can be used.
6. Any vLLM or LLM-d plugin should be implemented in Python with C acceleration as needed.
7. In the event of power-off events at the system level (e.g. node is powered off) it is acceptable to recover as much as possible of the cache state. There are no constraints on how much should be recovered.
8. The Certus system should run as a separate process from Python and the vLLM clients.  IPC mechanisms and shared memory can be used for communication.
9. Multiple clients should be able to share the same cache space (corresponding to a Certus instance).
10. Multiple instances of Certus can operate on the same host and flexibly partition both DRAM and SSD resources.
11. If a client crashed, the Certus server should release the session (potentially after some timeout) and continue to serve other clients.
12. The Certus server will be allocated at least one SSD (either SPDK device or kernel block device).
13. When allocating multiple SSD devices to Certus, all drives are assumed to be identical (same model, make, performance, capacity).
14. Certus should be NUMA-aware (configurable) and allow deployment in a system that support multiple NUMA zones (e.g. two socket system).
15. Data should be read-after-write consistent.
16. Multiple writes to the same cache block (identified by the key) are considered a failure.  Such a scenario should be logged and either raised as a panic or implicitly over-written.
