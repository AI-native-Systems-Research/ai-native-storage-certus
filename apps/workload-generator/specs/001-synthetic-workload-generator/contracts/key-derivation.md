# Contract: Key Derivation

**Version**: 1
**Status**: Draft
**Normative for**: `workload-model`, and any tool that needs to verify or
reproduce a generated trace's keys.

A block's `CacheKey` is a `u64` derived from the block's identity **and its
parent's key**. This document specifies the derivation exactly, because two
independent programs must be able to compute the same key: the generator that
issues it, and any consumer that checks a trace it did not produce.

## Why this is specified rather than left to the implementation

Certus treats `CacheKey` as an opaque `u64`
(`components/interfaces/src/idispatch_map.rs:6`), so the generator is free to
choose any key function. That freedom is exactly why the choice must be pinned:

- **Cross-node consistency (spec FR-029).** The same object must have the same
  key on every node, or a remote hit is impossible.
- **Cross-toolchain reproducibility (spec FR-034, FR-072).** A plan must be
  byte-identical on any machine and any compiler version.

Both rule out the convenient options.
`std::collections::hash_map::DefaultHasher` (SipHash) is not stable across Rust
versions. `ahash` is not stable across its own versions. Either would produce
keys that differ between toolchains while appearing to work, which is the worst
failure mode available: a silent loss of every cross-node hit.

## The mix function

`splitmix64`, verbatim, on a 64-bit state:

```text
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
```

All arithmetic is wrapping. All shifts are logical. No endianness question
arises, because the input and output are `u64` values, never bytes.

## Chaining

Every key is derived from its parent. A chain root uses parent `0`.

```text
key(parent, salt) = splitmix64(splitmix64(parent) ^ salt)
```

The inner mix on `parent` is not redundant: it prevents a low-entropy salt from
leaving structure from the parent visible in the child, which would let two
different chains collide in their low bits.

## Salts

`salt` identifies the block *within its role*, and the salt space is
partitioned so that no two block kinds can collide by construction.

| Block kind | Salt |
| --- | --- |
| Shared-object block | `(SHARED_TAG << 48) ^ (class_id << 32) ^ (instance_index << 16) ^ block_ordinal` |
| Session input block | `(INPUT_TAG << 48) ^ (session_id << 16) ^ block_ordinal` |
| Session output block | `(OUTPUT_TAG << 48) ^ (session_id << 16) ^ block_ordinal` |

with `SHARED_TAG = 1`, `INPUT_TAG = 2`, `OUTPUT_TAG = 3`.

`class_id` is the shared class's **declaration index** in the description file,
not a hash of its name — so renaming a class does not change keys, but
reordering declarations does. That is the intended trade: declaration order is
already semantic (spec FR-028).

## Chain construction

A session's chain is built in exactly this order, and the order *is* the
sharing mechanism (spec FR-027, FR-028):

1. **Shared prefix.** For each `uses` entry in declaration order, for each
   chosen instance in ascending `index` order, for each block ordinal
   `0..length_blocks`: append `key(tip, shared_salt)`.
2. **Per turn**, for `turn = 0..turns_total`: append the turn's input blocks,
   then its output blocks, each chaining onto the running tip.

**Consequences that follow from this and are load-bearing:**

- Two sessions share exactly the leading run of blocks for which every
  preceding choice matched. An object held in common at a differing position
  contributes nothing.
- Because instances are sorted by index, the chain is a function of the chosen
  *set*, so overlapping sets give **nested** chains. `{0,1}` versus `{0,1,4}`
  shares the first two objects; unsorted, the same sets could share nothing.
- Growth blocks chain onto a session-unique prefix, so they are private by
  construction (spec FR-030) and can only ever be reused intra-session.
- A `uses` count of 0 for a class is not a no-op: it changes what the next
  class's blocks chain onto, and therefore diverges the chain. This is the
  mechanism by which count distributions build a prefix *tree*.

## Test vectors

Normative. An implementation that does not reproduce these is wrong. Values are
hexadecimal `u64`.

```text
splitmix64(0)                    = e220a8397b1dcdaf
splitmix64(1)                    = 910a2dec89025cc1
splitmix64(0xffffffffffffffff)   = e4d971771b652c20

key(parent=0, salt=0)            = splitmix64(splitmix64(0) ^ 0)
key(parent=0, salt=1)            = splitmix64(splitmix64(0) ^ 1)
```

The implementation MUST ship a unit test asserting the three `splitmix64`
values above, and a test that a two-element chain built from `(parent=0,
salt=1)` then `(parent=that, salt=2)` is stable across runs. The concrete chain
values are to be pinned by that test on first implementation and then never
changed — changing them invalidates every previously generated trace.

## Collision posture

The key space is 64-bit and the design target is 10M live keys (spec SC-012),
so the birthday-bound collision probability is about `10^14 / 2^65` ≈ 3 × 10^-6
per run. Collisions are therefore not handled: a collision would appear as an
unexpected cache hit, and at that rate it cannot bias a measurement. This is a
deliberate decision rather than an oversight, and it is the reason the key
width does not need to grow with the scale target.
