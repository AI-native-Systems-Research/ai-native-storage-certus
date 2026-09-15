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
| Shared-object block | `(SHARED_TAG << 62) ^ (class_id << 50) ^ (instance_index << 24) ^ block_ordinal` |
| Session input block | `(INPUT_TAG << 62) ^ (session_id << 24) ^ block_ordinal` |
| Session output block | `(OUTPUT_TAG << 62) ^ (session_id << 24) ^ block_ordinal` |

with `SHARED_TAG = 1`, `INPUT_TAG = 2`, `OUTPUT_TAG = 3`. Tag `0` is unused and
reserved.

`class_id` is the shared class's **declaration index** in the description file,
not a hash of its name — so renaming a class does not change keys, but
reordering declarations does. That is the intended trade: declaration order is
already semantic (spec FR-028).

### Field widths, and why they are what they are

The shifts above are not arbitrary spacing: each field's **width is the gap to
the next field**, and an overflowing field would silently alias two different
blocks onto one key. So the widths are part of this contract.

| Field | Bits | Width | Ceiling |
| --- | --- | --- | --- |
| tag | 62–63 | 2 | 3 kinds (`0` reserved) |
| `class_id` | 50–61 | 12 | 4 096 shared classes |
| `instance_index` | 24–49 | 26 | 67 108 864 live instances per pool |
| `block_ordinal` | 0–23 | 24 | 16 777 216 blocks per instance or stream |
| `session_id` | 24–61 | 38 | 274 877 906 944 sessions per run |

Both layouts use all 64 bits exactly: `2 + 12 + 26 + 24` for a shared block and
`2 + 38 + 24` for a session block. Every ceiling is orders of magnitude beyond
the scale target of 10 000 concurrent sessions and 10 000 000 live keys (spec
SC-012), so none is reachable by a description anyone would write.

**An earlier revision of this contract placed the tag at bit 48 and gave every
field 16 bits.** That spent 16 bits on a 3-value tag while capping
`block_ordinal` at 65 536 — reachable, since a session's input stream grows every
turn and `turns × E[input_growth]` passes it in a long run — and
`instance_index` at 65 536, which a document pool written as `size: 100000`
would exceed. Both would have surfaced only as an inexplicable cache hit. The
tag was moved to the top 2 bits and the 14 freed bits given to the two fields
that needed them. This changed every salt-derived key, which was acceptable only
because no trace had yet been generated; see *Versioning* below.

An implementation MUST reject an out-of-range coordinate rather than truncating
it. Because the salt is assembled with XOR, truncation does not saturate — it
corrupts the neighbouring field.

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

key(parent=0, salt=0)            = a706dd2f4d197e6f
key(parent=0, salt=1)            = 08b4fda8c892b50e
key(parent=08b4fda8c892b50e, salt=2)
                                 = bedb5bf1cd5ec111
```

Those five depend only on the mix function and the chain rule, so they are
unaffected by the salt layout. The three below pin the **layout** as well, and a
change to any field offset would alter them while leaving everything above
intact:

```text
key(0, shared_salt(class=0, instance=0, ordinal=0))  = fb269438518a37a0
key(0, input_salt(session=7, ordinal=3))             = 53305e2821f04364
key(0, output_salt(session=7, ordinal=3))            = 624741cd5024ca0e
```

The salt encodings themselves, as three probes with every field distinct:

```text
shared_salt(class=0xabc, instance=0x123456, ordinal=0xdef012)
                                 = 6af0123456def012
input_salt(session=0x12345678, ordinal=0x9abc)
                                 = 8012345678009abc
output_salt(session=0x12345678, ordinal=0x9abc)
                                 = c012345678009abc
```

The implementation MUST ship unit tests asserting all of the above, and MUST
assert each field's width so that an out-of-range coordinate panics rather than
aliasing.

## Versioning

The concrete values above are pinned and must not change: changing them
invalidates every previously generated trace and silently destroys every
cross-node hit. They were revised exactly once, when the field widths were
rebalanced (see *Field widths* above), and that was only defensible because no
trace existed yet.

If the key function ever has to change once traces exist, the change is a **new
version**, not an edit: the old function stays, the trace manifest records which
version produced it, and a consumer refuses a trace whose version it cannot
compute. Editing these values in place is never the answer, because the failure
it produces is invisible — a trace that loads, replays, and quietly misses.

## Collision posture

The key space is 64-bit and the design target is 10M live keys (spec SC-012),
so the birthday-bound collision probability is about `10^14 / 2^65` ≈ 3 × 10^-6
per run. Collisions are therefore not handled: a collision would appear as an
unexpected cache hit, and at that rate it cannot bias a measurement. This is a
deliberate decision rather than an oversight, and it is the reason the key
width does not need to grow with the scale target.

A **field overflow** is a different thing and is not tolerated. A birthday
collision is a random event at a rate of 10^-6 per run and cannot bias a
measurement. An overflowing coordinate aliases two specific blocks
*systematically*, on every run, for as long as that description is in use. The
first is accepted; the second panics.
