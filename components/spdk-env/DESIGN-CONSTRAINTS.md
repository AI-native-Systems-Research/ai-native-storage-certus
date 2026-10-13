# spdk-env: design constraints

This file gives the rules of `spdk-env` that other components depend on.
If a change breaks one of these rules, all server threads can run on one CPU, or the process can crash at shutdown. Also, the NVMe driver can fail to attach the drives.
The rules for all of Certus, the status values and the change procedure are in [knowledge/design-constraints.md](../../knowledge/design-constraints.md).

Spec: `specs/002-spdk-env-vfio-init/spec.md` (cited as "spec"). Spec `002` replaces the spec `001-spdk-vfio-env`.

## ENV-1. Give the caller its CPU affinity back after `spdk_env_init`

- **Rule:** Before `spdk_env_init`, save the CPU affinity of the thread that calls it. After `spdk_env_init`, set it again, also when the init fails.
- **Reason:** DPDK EAL init pins the thread that calls it to its main lcore (CPU 0 with the default core mask). Each thread that the server starts after that gets the same single CPU. See DC-4 in the main document.
- **Applies to:** `src/env.rs` and each other code that calls `spdk_env_init` or `rte_eal_init`.
- **Status:** Established by spec (FR-022) and by code (commit `d102a00b`, Certus PR #507).
- **Evidence:** spec line 163; `src/env.rs:91-102`, `src/env.rs:116-139`.

## ENV-2. Free all SPDK DMA buffers before `fini()`

- **Rule:** Drop each `DmaBuffer` that SPDK allocated before you call `fini()`.
- **Reason:** After `fini()`, `DmaBuffer::drop` does not call the deallocator, because a call into SPDK can then crash. The memory stays allocated until the process stops.
- **Applies to:** server shutdown, `memory-tier`, `block-device-spdk-nvme`, each owner of a `DmaBuffer`.
- **Status:** Established by spec (FR-019, FR-020). Note: the same check also stops the free of buffers that do not come from SPDK (`DmaBuffer::from_raw`) when SPDK was never started.
- **Evidence:** spec lines 124 and 133; `components/interfaces/src/spdk_types.rs:375-390`; `src/env.rs:222-234` (`do_fini` clears the flag before `spdk_env_fini`).

## ENV-3. Enumeration must not attach the devices

- **Rule:** In the enumeration callback, return a non-zero value so that SPDK does not claim the device.
- **Reason:** `block-device-spdk-nvme` attaches each controller later with `spdk_nvme_probe`. A device that the enumeration claims is not available to it.
- **Applies to:** `src/env.rs` enumeration.
- **Status:** Established by spec (FR-006).
- **Evidence:** spec line 105; `src/env.rs:194-195`.

## ENV-4. Call `init()` before the block devices start

- **Rule:** Call `ISPDKEnv::init()` before you call `initialize()` on any `block-device-spdk-nvme` instance.
- **Reason:** The block device examines only the connection of the `spdk_env` receptacle. It does not examine that the environment is active.
- **Applies to:** the server applications, `dispatcher` (which creates the block devices).
- **Status:** Established by code (the order in the applications). **Code and documentation disagree:** the documentation of the block device states that it returns an error if the environment is not active.
- **Evidence:** `components/block-device-spdk-nvme/src/lib.rs:124-136`; `apps/certus-server/src/main.rs:316-321` (`init()` before the dispatcher starts).
