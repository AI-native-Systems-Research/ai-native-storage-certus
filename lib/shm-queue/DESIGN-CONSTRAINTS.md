# shm-queue: design constraints

This file gives the rules of `shm-queue` that the Python client (`certus-shmq-connector/certus_shmq_connector/ring.py`) and `shmq-dispatcher` depend on.
If a change breaks one of these rules, the server can miss a request, or a client can read a reply that is not complete. A client can also wait for all time.
The rules for all of Certus, the status values and the change procedure are in [knowledge/design-constraints.md](../../knowledge/design-constraints.md).

This crate has no spec. The module documentation in `src/lib.rs:1-34` and the module documentation of `ring.py` (lines 1-22) are the contract.

## SHMQ-1. Write `seq` last, and run only on x86-64

- **Rule:** To send a message, write the payload, the opcode and the length first. Write `seq` last, with `Release` order in Rust. Do not run the Python client on a CPU with weak memory order.
- **Reason:** Python cannot make a memory fence. The design is correct only because x86 keeps the order of stores.
- **Applies to:** `src/lib.rs` (client and server), `ring.py`.
- **Status:** Established by code. Both sides document it, and `ring.py` refuses to start on a different architecture.
- **Evidence:** `src/lib.rs:23-29`, `src/lib.rs:569-580`; `ring.py:11-16`, `ring.py:119-125`.

## SHMQ-2. Change the shared-memory layout on both sides, and increase `ABI_VERSION`

- **Rule:** When you change the header, the control block offsets or the channel layout, make the same change in `ring.py`. Increase `ABI_VERSION` on both sides.
- **Reason:** The Rust server and the Python client read the same bytes. The client refuses to attach only if `ABI_VERSION` is different. Nothing increases the value automatically.
- **Applies to:** `src/lib.rs`, `ring.py`.
- **Status:** Established by code. `ABI_VERSION` is 1 on both sides.
- **Evidence:** `src/lib.rs:46-48`, `src/lib.rs:466-472`; `ring.py:18-21`, `ring.py:38-42`, `ring.py:452-456`.

## SHMQ-3. One request in flight and one owner for each channel

- **Rule:** Send one request on a channel, and wait for its reply before you send the next one. Each client thread must own its channel through the owner word. Each process must claim only channels in its own slice.
- **Reason:** A channel is a mailbox with depth 1. The Python claim is a read and then a write, which is safe only inside one process. Two processes in the same slice can claim the same channel.
- **Applies to:** `src/lib.rs` (Rust client claim), `ring.py` (Python claim), the connector `spec.py` (slice for each rank).
- **Status:** Established by code. Both sides document it.
- **Evidence:** `src/lib.rs:13-15`; `ring.py:526-552`; `certus-shmq-connector/certus_shmq_connector/spec.py:194-206`.

## SHMQ-4. Use the shared futex

- **Rule:** Use `FUTEX_WAIT` and `FUTEX_WAKE`, not the `_PRIVATE` forms.
- **Reason:** The client and the server are different processes. A private futex does not wake a different process, so the client waits until its time-out.
- **Applies to:** `src/lib.rs`, `ring.py`.
- **Status:** Established by code. The module documentation states it.
- **Evidence:** `src/lib.rs:31-34`.

## Known gaps

- The documentation of `Header::generation` states that the server increases it at each create, so that a client can find a server restart. The server always writes 1. Thus a client cannot find a server restart through this field. Evidence: `src/lib.rs:66-72` and `src/lib.rs:326-329`.
