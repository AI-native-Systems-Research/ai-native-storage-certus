# Extent Bitmap Consistency

## Scope

Single component: `extent-manager` (RegionState / AllocationBitmap / Slab)

## Description

Verifies that concurrent reserve/publish/remove operations on the extent
manager's bitmap allocator never leave the bitmap in an inconsistent state.
Models a single `RegionState` with a fixed-size slab. Multiple threads perform
the two-phase allocation protocol (reserve → publish or abort) interleaved with
remove operations. The write lock (`parking_lot::RwLock`) is modeled as
`atomic{}` blocks, ensuring each critical section is indivisible.

The core invariant is: after every operation sequence, the bitmap's set-bit
count matches `allocated_count`, no slot is simultaneously allocated-and-free,
and no slot is published without being allocated.

## Properties Verified

| ID | Property                                                         | Type   |
| -- | ---------------------------------------------------------------- | ------ |
| P1 | No slot is bitmap-allocated with FREE_KEY unless a WriteHandle owns it | Safety |
| P2 | alloc_slot never returns a slot whose bitmap bit is already set  | Safety |
| P3 | free_slot is never called on a slot whose bitmap bit is clear    | Safety |
| P4 | remove_extent only succeeds on bitmap-set slots with published key | Safety |
| P5 | allocated_count equals actual set-bit count at termination       | Safety |

## System Abstraction

| Real component                          | Promela element                  |
| --------------------------------------- | -------------------------------- |
| `RegionState` write lock                | `atomic{}` blocks                |
| `AllocationBitmap.words[]`              | `bool bitmap[N_SLOTS]`           |
| `AllocationBitmap.allocated_count`      | `byte allocated_count`           |
| `Slab.keys[]`                           | `byte keys[N_SLOTS]`            |
| `Slab.rover`                            | `byte rover`                     |
| `WriteHandle` ownership                 | `byte owner[N_SLOTS]`           |
| `reserve_extent` → `alloc_extent()`    | `alloc_slot()` inline            |
| `WriteHandle::publish()`                | `publish_slot()` inline          |
| `WriteHandle::drop()` (abort)           | `free_slot()` inline             |
| `remove_extent_by_offset()`             | Scan + `free_slot()` in atomic   |

## Assumptions / Stubs

- **Single region**: Multi-region adds no new interleavings (each region
  has its own independent write lock). Verifying one region suffices.
- **Fixed slab**: The slab is pre-allocated with N_SLOTS capacity. Dynamic
  slab creation/destruction via BuddyAllocator is not modeled (orthogonal
  to bitmap consistency within a slab).
- **Immediate free on remove**: Real code defers bitmap clear to
  `flush_pending_frees()` at checkpoint time. The model frees immediately,
  which is a safe over-approximation (allows more interleavings).
- **No I/O**: AllocationBitmap is purely in-memory; no disk persistence
  modeled (checkpoint correctness is a separate property).

## Running

```bash
# Safety verification
make safety

# Or manually:
spin -a extent_bitmap_consistency.pml
cc -O2 -DSAFETY -o pan pan.c
./pan -m200000
```

## Tuning the Model

| Parameter      | Value | Rationale                                           |
| -------------- | ----- | --------------------------------------------------- |
| N_THREADS      | 3     | Minimum to expose 3-way contention on shared slots  |
| N_SLOTS        | 4     | Small slab — forces slot reuse after free           |
| OPS_PER_THREAD | 2     | Each thread does 2 reserve cycles → exercises reuse |

The defaults produce a tractable state space. To stress-test with more reuse:

```bash
spin -DN_THREADS=4 -DOPS_PER_THREAD=3 -a extent_bitmap_consistency.pml
cc -O2 -DSAFETY -DMEMLIM=8192 -o pan pan.c
./pan -m500000
```

## Correspondence to Source Code

| Model location                  | Source file                                  | Line range |
| ------------------------------- | -------------------------------------------- | ---------- |
| `alloc_slot()` inline           | `components/extent-manager/src/slab.rs`      | 29–34      |
| `free_slot()` inline            | `components/extent-manager/src/slab.rs`      | 37–39      |
| `publish_slot()` inline         | `components/extent-manager/src/region.rs`    | 117–122    |
| `Thread` reserve phase          | `components/extent-manager/src/lib.rs`       | 516–528    |
| `Thread` publish/abort choice   | `components/extent-manager/src/lib.rs`       | 533–549    |
| `Thread` remove phase           | `components/extent-manager/src/region.rs`    | 124–153    |
| `bitmap[].set/clear`            | `components/extent-manager/src/bitmap.rs`    | 17–30      |
| `find_free_from` (rover scan)   | `components/extent-manager/src/bitmap.rs`    | 42–50      |
