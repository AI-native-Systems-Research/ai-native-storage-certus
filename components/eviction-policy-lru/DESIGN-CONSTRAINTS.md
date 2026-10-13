# eviction-policy-lru: design constraints

This file gives the rules of `eviction-policy-lru` that `dispatch-map` and `memory-tier` depend on.
If a change breaks one of these rules, entries can stay in the cache, locks can deadlock, or the process can panic.
The rules for all of Certus, the status values and the change procedure are in [knowledge/design-constraints.md](../../knowledge/design-constraints.md).

Spec: `specs/001-lru-eviction-policy/spec.md` (cited as "spec").

## LRU-1. Do not use a handle after the policy removes its node

- **Rule:** After `remove`, a pop from `evict_candidates`, or `clear_pool`, do not use that `EvictionHandle` again.
- **Reason:** A handle is only a pool and a node index, with no generation. The free list gives the index to the next `track`. A stale handle then changes the node of a different key. After `clear_pool`, the node list is empty, and a stale handle causes an index panic.
- **Applies to:** `dispatch-map`, `memory-tier`, each other caller.
- **Status:** Established by code. **Code and spec disagree:** spec FR-010 states that a stale handle has no effect. That is true only until a new `track` gets the same index.
- **Evidence:** spec FR-010 (line 66), FR-011 (line 68), implementation note (line 107); `src/lib.rs:70` (handle = pool + index); `src/lru_list.rs:36-45` (index used again); `src/lru_list.rs:145-150` (`clear`); `src/lru_list.rs:70-73`, `src/lru_list.rs:120-123` (index without a bounds check).

## LRU-2. Do not track a key that is already tracked

- **Rule:** Call `track` for a key only when the pool does not have that key.
- **Reason:** `track` always adds a new node. A second node for the same key is never removed by the owner, so the policy gives that key as a candidate again later.
- **Applies to:** `dispatch-map` (`recover_extent`, `create_memory_tier_entry`, `initialize`), `memory-tier` (`insert`).
- **Status:** Established by spec (FR-002).
- **Evidence:** spec line 58; `src/lib.rs:53-70`; the guards in callers: `components/dispatch-map/src/lib.rs:577-580`, `components/memory-tier/src/lib.rs:360-367`. The `initialize()` walk in `dispatch-map` has no guard (`components/dispatch-map/src/lib.rs:86-106`).

## LRU-3. The policy does not know about pins, and it must not call its users

- **Rule:** Treat each candidate as a suggestion. The caller must examine pins and SSD state before it frees anything. Do not call `dispatch-map` or `memory-tier` from the policy.
- **Reason:** A candidate can have a pin. Its data can also be not yet on the SSD. The users call the policy when they hold their own locks (see DMAP-7 and MT-3).
- **Applies to:** this component and each new `IEvictionPolicy` implementation.
- **Status:** Established by code.
- **Evidence:** `components/dispatch-map/src/lib.rs:86-92`; `components/memory-tier/src/lib.rs:360-367`; `components/dispatcher/src/lib.rs:1066-1071` (the caller examines the candidate).
