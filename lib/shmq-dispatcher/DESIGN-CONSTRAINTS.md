# shmq-dispatcher: design constraints

This file gives the rules of `shmq-dispatcher` that the connector and the dispatcher depend on.
If a change breaks one of these rules, the Python client can decode a reply incorrectly, or vLLM can use a block that did not load. A client can also wait for all time.
The rules for all of Certus, the status values and the change procedure are in [knowledge/design-constraints.md](../../knowledge/design-constraints.md).

This crate has no spec. The module documentation (`src/lib.rs:1-20`, `src/wire.rs:1-81`) and `components/dispatcher/specs/002-served-by-tier-attribution/contracts/served-by.md` are the contract.

## SHMD-1. Change opcodes and message layouts on both sides

- **Rule:** If you add or change an opcode or a message layout in `src/wire.rs`, change `ring.py` in the same way. Update the opcode tests on both sides.
- **Reason:** The Python client encodes and decodes the same bytes, and nothing negotiates a version. A client and a server with different layouts read incorrect values. An old server refuses a new opcode (for example `TOUCH_CHECK` = 16).
- **Applies to:** `src/wire.rs`, `src/translate.rs`, `certus-shmq-connector/certus_shmq_connector/ring.py`.
- **Status:** Established by code. Both files state it.
- **Evidence:** `src/wire.rs:3-7`, `src/wire.rs:84-101`, `src/wire.rs:300-322` (test); `ring.py:18-21`, `ring.py:66-81`; `certus-shmq-connector/tests/test_wire.py:50-52`.

## SHMD-2. A Lookup reply byte of 0 means "not served". Any other value means "delivered"

- **Rule:** In a Lookup reply, write a non-zero byte only for a key whose data is in the GPU buffer. Do not send the `PENDING` state on Lookup. Keep the three Check states `MISS` = 0, `RESIDENT` = 1, `PENDING` = 2.
- **Reason:** The connector tests `byte != 0`. A non-zero byte for a key that was not delivered makes vLLM use an empty block. A load of a `PENDING` key fails, and vLLM stops.
- **Applies to:** `src/translate.rs`, `src/wire.rs`, the connector decoders.
- **Status:** Established by spec (dispatcher spec 002 FR-020, `contracts/served-by.md`) and by code. The comment on the Lookup reply in `src/wire.rs:36-37` still shows `[ok:u8]`.
- **Evidence:** `src/translate.rs:692-710`; `src/wire.rs:103-116`; `components/dispatcher/specs/002-served-by-tier-attribution/spec.md:527`.

## SHMD-3. Each request gets exactly one reply. After a panic, reply and then stop the process

- **Rule:** Make each worker write exactly one reply for each request: on success, on error and on panic. After a panic, write the error reply and then call `std::process::abort()`.
- **Reason:** A request without a reply makes the client wait until its deadline. A process that continues after a panic can serve data from an extent that is only partly written.
- **Applies to:** `src/serve.rs` workers.
- **Status:** Established by code.
- **Evidence:** `src/serve.rs:186-238`.

## SHMD-4. The reservation time-out must be longer than each store

- **Rule:** Set `--reserve-timeout-secs` to a value that is longer than the longest Reserve → CopyToStore → CommitStore sequence.
- **Reason:** The reaper releases each reservation that is older than the time-out. A later CommitStore for that key then fails. The connector reports all stores as a success, so the cache loses the block without an error.
- **Applies to:** `src/translate.rs` reaper, the server arguments.
- **Status:** Established by code. The default time-out (30 s) is the same as the default client deadline (30 s).
- **Evidence:** `src/translate.rs:374-395`; `src/translate.rs:560-566`; `apps/certus-server/src/main.rs:184-185`; `certus-shmq-connector/certus_shmq_connector/ring.py:372-376`.

## SHMD-5. One poller thread owns the request scan

- **Rule:** Let only the poller thread read the request words and keep `last_seen`. Give each request to the worker pool. Do not run a Lookup on the poller thread.
- **Reason:** The poller does not stop between requests. A slow operation on the poller stops all other channels for that time.
- **Applies to:** `src/serve.rs`.
- **Status:** Established by code. Note: the poller spins until shutdown, with no limit and no switch to stop the spin. See P-3 in the main document.
- **Evidence:** `src/lib.rs:8-20`; `src/serve.rs:326-340`.
