# Spec Sync — Align Tasks
Project: dispatcher-p2p
Source drift cycle: `drift-report.json` / `drift-report.md` (Spec-Sync Phase B, 2026-08-20)

Items below are real code-vs-spec defects: the spec requirement is correct and agreed,
the code does not meet it. Per Phase B policy these are resolved by an ALIGN task (fix the
code to match the spec) — **no `.rs` source is modified by this sync pass**.

---

## Task: Align 001-gpudirect-cold-path/FR-017 — ✅ RESOLVED 2026-09-03

> **Resolved 2026-09-03 (Spec-Sync ALIGN apply).** Introduced a shared free
> helper `publish_eviction(tx, dropped, key, reason)` in `src/lib.rs` that
> increments the drop counter on **any** non-delivery (full channel **or** no
> subscriber) when `dropped` is `Some`, and is a no-op when `dropped` is `None`.
> All four live emit sites now route through it — the three in
> `evict_for_space_inner` (reached via `evict_for_space_emit` and the
> `promote_to_memory_tier` scoped-thread path), `BackgroundEvictor::evictor_loop`,
> and `MemoryTierEvictor::evictor_loop`. The internal, deliberately non-emitting
> `evict_for_space` path passes `dropped = None` (neither publishes nor counts).
> `eviction_dropped` was widened `AtomicU64` → `Arc<AtomicU64>` so the two
> background evictor threads share the same counter (threaded in via
> `BackgroundEvictor::start` / `MemoryTierEvictor::start`); the dead
> `emit_eviction` was deleted. Two unit tests added
> (`publish_eviction_counts_only_undeliverable_emits`,
> `eviction_dropped_count_tracks_emit_path`). Verified: `cargo build` clean,
> `cargo clippy -p dispatcher-p2p --all-targets -- -D warnings` clean for
> dispatcher-p2p's sources, `cargo test -p dispatcher-p2p` 71 passed / 0 failed.

**Severity**: Moderate

**Spec Requirement**: FR-017 requires that when an `EvictionEvent` cannot be delivered (channel
full, or no subscriber registered) "the event MUST be silently dropped **and counted**, and the
running drop count MUST be readable and reset via `eviction_dropped_count()`."

**Current Code**: The drop counter (`eviction_dropped.fetch_add`) is incremented in exactly one
place — `emit_eviction` (`src/lib.rs:228-236`), which is annotated `#[allow(dead_code)]` and has
**no call sites**. Every live eviction path publishes via a bare `let _ = tx.try_send(...)` that
discards the `Err` without incrementing the counter:
- `evict_for_space_inner` / `evict_for_space_emit` — `src/lib.rs:602-607, 618-623, 633-645`
- `BackgroundEvictor::evictor_loop` — `src/background.rs:414-419`
- `MemoryTierEvictor::evictor_loop` — `src/background.rs:611-616`

As a result `eviction_dropped_count()` (`src/lib.rs:224-226`) always returns 0. The channel /
`try_send` / non-blocking / silent-drop parts of FR-017 are correctly implemented; only the
**drop-count** guarantee is unmet.

**Required Change**: Make every live eviction publish site increment `eviction_dropped` on a
failed `try_send`, so the running count reflects reality. Route all three call sites (inline
`evict_for_space_*`, `BackgroundEvictor`, `MemoryTierEvictor`) through a single shared helper that
performs `try_send` and, on `Err`, `eviction_dropped.fetch_add(1, Relaxed)` — then either promote
`emit_eviction` into that shared helper (removing `#[allow(dead_code)]`) or delete it if the new
helper supersedes it. Note the background evictors currently hold `Sender<EvictionEvent>` clones
directly, so the shared counter (`Arc<AtomicU64>`) must be threaded into those threads at
construction (`BackgroundEvictor::start` / `MemoryTierEvictor::start`) alongside the sender.

**Files to Modify**: `components/dispatcher-p2p/src/lib.rs` (eviction publish sites + counter
plumbing), `components/dispatcher-p2p/src/background.rs` (`BackgroundEvictor` and
`MemoryTierEvictor` publish sites + shared counter injection).

**Estimated Effort**: Small–Medium (single shared helper + threading an `Arc<AtomicU64>` into two
background threads; add a unit test that fills a capacity-1 channel and asserts
`eviction_dropped_count()` is non-zero, then zero after reset).

### Acceptance Criteria

- [ ] All live eviction publish sites (inline `evict_for_space_*`, `BackgroundEvictor`,
      `MemoryTierEvictor`) increment `eviction_dropped` when `try_send` fails.
- [ ] `eviction_dropped_count()` returns the true number of dropped events since the last call and
      resets to 0 on read (existing swap semantics preserved).
- [ ] No remaining `#[allow(dead_code)]` eviction-emit path that silently bypasses the counter.
- [ ] A test drives evictions with a full (or unregistered) channel and asserts the drop count is
      non-zero, then zero after a subsequent read.
- [ ] Non-blocking / silent-drop behavior of eviction delivery is unchanged (eviction never blocks
      or fails on a full channel).
