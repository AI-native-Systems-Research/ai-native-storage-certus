# Contract: Workload Model YAML Schema

**Version**: 1
**Status**: Draft
**Consumers**: `certus-workload plan | report | emit`, `certus-trace fit | validate | convert`,
`certus-workload-run run`

This is the normative reference for the generator's input. It is the contract that keeps the
input compact: the file holds *fitted statistical parameters*, never an access trace.

## Design rules

1. **Four orthogonal sections.** `corpus` (what keys exist and how they overlap), `workload`
   (who asks for what, when), `topology` (which node asks for what), `run` (execution and
   measurement). Changing one axis never requires editing another. This factoring is what
   prevents combinatorial enumeration.
2. **One distribution syntax everywhere** (§ Distributions).
3. **Each kind of reuse is specified in exactly one place.** There are two, and the schema
   keeps them apart because they are physically different phenomena:
   - **Inter-session** sharing — different sessions walking the same trunk — lives only in
     `corpus.trees` (`roots.popularity` for *which* trunk, `shared_depth` for *how far down*
     it is shared, `branching`/`branch_skew` for the trunk's shape).
   - **Intra-session** sharing — turn N+1 re-reading turn N's blocks — lives only in
     `workload.sessions` (`turns`, `growth_per_turn`), along with the session's own private
     path length (`private_depth`).

   So `corpus` describes the **shared** key structure and `workload` describes how a session
   **traverses** it and how far past it that session goes alone. A top-level `gets_per_key` or
   `lifetime` is a schema error, and so is restating a *shared* quantity — `popularity` or
   `shared_depth` — inside `workload.mix`, because the two statements could silently disagree.
   Path length is stated once, by the formula in § `workload`.
4. **No puts.** The model describes *requests*; populates are whatever the system missed.
   Specifying puts would assume the hit rate the experiment measures.
5. **Unknown fields are errors**, so a mistyped distribution parameter cannot silently take
   a default.
6. **The document describes a workload, never a storage system.** There is deliberately no
   `system:` section and no mention anywhere of tiers, caches, memory, or disks. A workload is
   a statistical statement about *which blocks are asked for, by whom, in what order, at what
   size* — and it must mean exactly the same thing whether the consumer is Certus with two
   tiers, Certus with five, a single flat cache, a simulator, or a plain file on disk. Anything
   describing the system under test — capacities, eviction policy, watermarks, pinning, tier
   placement — is a property of *that consumer*, supplied by the consumer's own configuration
   or command line, and never of the workload. What the generator publishes instead is the
   **realised working-set size** over `run.wss_window` (§ `run`), which is a workload
   statistic: a consumer that wants a cache sized to a quarter of the working set can compute
   that from the report without the workload ever having named a cache.
7. **The document describes a workload, never a tool operation.** Parameters governing what the
   tool *does to* a model — `fit`/`validate` tolerances above all — are command-line options, not
   schema fields (spec FR-057b). There is deliberately no `fit:` section: two documents with
   identical workload content must compare equal, and a tolerance embedded here would make them
   differ while describing the same workload. Adding one later would breach rule 1.

## Top-level document

```yaml
version: 1                  # required; generator refuses versions it does not implement
seed: 0xC0FFEE              # required; every random draw derives from this
extends: presets/conv.yaml  # optional; deep-merged, this document wins
# Run length. Exactly one of duration | requests | blocks | unbounded.
#   duration / requests / blocks  -- any output mode
#   blocks                        -- REQUIRED for a file output mode: it is the only
#                                    one that converts directly to a file size, since
#                                    request length is drawn per session
#   unbounded: true               -- direct-to-server ONLY; rejected for file modes
duration: 120s
requests: 2_000_000
blocks: 50_000_000
corpus:   {...}             # required
workload: {...}             # required
topology: {...}             # optional; omitted ⇒ a single node asks for everything
run:      {...}             # required
sweep:    {...}             # optional
```

### Units

Durations accept `ns|us|ms|s|m|h`. Sizes accept `B|KiB|MiB|GiB` (binary) and `KB|MB|GB`
(decimal). Rates accept a `/s` suffix. Bare integers may use `_` separators. Fractions are
plain floats in `[0, 1]` unless stated otherwise.

**A suffix is resolved against the field, not the value.** A bare `3` is three *seconds* under
`think_time` and three *bytes* under `block_bytes`, so the unit comes from where the scalar sits.
Suffixed scalars are therefore rewritten into each field's base unit — bytes for sizes, seconds for
times — as part of normalisation, after the `extends` merge and before anything reads the document.
Two consequences worth stating:

- **A spelling is not a workload.** `block_bytes: 128KiB` and `block_bytes: 131072` normalise to the
  same document, so they produce the same content hash and the same `--print-normalised` output. Two
  arms of a comparison cannot differ on punctuation alone.
- **A mistyped suffix is refused, naming the path.** `128QiB` is the same class of mistake as a
  mistyped field name, which the schema already rejects: a value silently read as `128` would give a
  run of the wrong size that reports itself as correct.

Within a distribution, only the parameters that *carry* the field's unit are converted: `value`,
`min`, `max`, `mean`, `stddev`, `median`, `scale`, and the first element of each `empirical` point.
`sigma`, `s`, `n`, `alpha` and a point's cumulative probability are dimensionless and are left
alone — `{median: 128KiB, sigma: 0.4}` is a size beside a bare ratio, since `sigma` is the standard
deviation of the *logarithm*.

## Distributions

Every distribution-valued field takes the same tagged union. A bare scalar is sugar for
`{dist: const, value: <scalar>}`.

| `dist` | Parameters | Notes |
| --- | --- | --- |
| `const` | `value` | |
| `uniform` | `min`, `max` | inclusive |
| `normal` | `mean`, `stddev` | truncated at 0 for non-negative fields; truncation is counted and reported |
| `lognormal` | `median`, `sigma` | the default shape for sizes, lengths, and think times |
| `exponential` | `mean` | |
| `geometric` | `mean` | discrete; the default for turn counts |
| `zipf` | `s`, `n` | `s` is the exponent; `n` the support size. The **discrete** pmf `p_k = k^-s / H_n(s)`, so every rank in `1..=n` has positive probability |
| `pareto` | `scale`, `alpha` | |
| `empirical` | `points: [[value, cum_prob], ...]` | what `fit` emits when no parametric shape fits well; linearly interpolated |

Integer-valued fields round half-to-even and clamp to their documented domain; every clamp is
counted and surfaced in the plan summary (never silently applied).

## `corpus` — what keys exist and how they overlap

```yaml
corpus:
  # Payload size per key. MUST be a pure function of key identity: the generator
  # derives the draw from the key's own hash, never from position in the stream,
  # because the dispatcher treats a size mismatch as a miss and a varying size for
  # one key would manufacture phantom misses.
  block_bytes: {dist: const, value: 128KiB}

  # A FOREST, not a single tree. Real KV traces show several independent prefix trees at
  # the top levels (many levels deep for RAG and multi-turn), each descending into
  # branches with no reuse at all at the bottom.
  trees:
    roots:
      count: 12                              # number of distinct depth-0 keys
      # Which root a SESSION binds to, drawn once per session and then sticky.
      # Root count << session count, so the binding is many sessions : one root.
      # The support size is `count`; supplying `n` here is a schema error.
      popularity: {dist: zipf, s: 0.9}

    # Depth at which INTER-session sharing ends: the length of the trunk this
    # session walks in common with other sessions bound to the same root.
    # This is the same quantity as the FR-056 prefix-sharing depth histogram, so a
    # fitted value and a validated value are measured the same way (see § Fitting).
    shared_depth: {dist: empirical, points: [[4, 0.10], [18, 0.75], [40, 1.0]]}

    # How the trunk WIDENS with depth, as a piecewise profile rather than one number.
    # Each entry says "from this depth onward, each trunk node has this many children",
    # until the next entry. Real tries are flat for long stretches and then fan out at
    # particular depths (see § Trunk width is piecewise, not smooth), so a single
    # exponent cannot describe them.
    #
    # MEANS, not distributions: child counts are integers and the mean is the only
    # moment that matters. A non-integer mean is realised exactly by giving each node
    # floor(m) or ceil(m) children with the probabilities that make E[children] = m.
    # Decided ONCE per node from that node's own identity plus the seed -- never from
    # arrival order (see below). Domain: >= 1 at every depth, so the trunk is unbounded
    # in depth and no session can run off the end of it.
    #
    # A fanout is a MEAN and the realised counts vary around it: a node with
    # fanout 1.18 gets 1 child with probability 0.82 and 2 with probability 0.18, so
    # the width at any depth is stochastic and converges on the stated value as more
    # of the trie is visited. The draw is keyed on the NODE, not on the visit, which
    # is what keeps a long run reproducible and independent of arrival order.
    #
    # `auto` solves for a profile that keeps sharing realisable (FR-009g). A bare
    # scalar is accepted as sugar for a single segment at depth 0, which is the old
    # `branch_factor` and still the right thing for a smoothly-branching trunk.
    branching: auto
    #   auto
    #   1.15                                     # sugar: uniform, all depths
    #   - {from_depth: 0,   fanout: 1.0}         # a flat shared preamble...
    #   - {from_depth: 12,  fanout: 40.0}        # ...one fanout event...
    #   - {from_depth: 13,  fanout: 1.0}         # ...a flat shared segment (a tool
    #                                            #    preamble common to that branch)
    #   - {from_depth: 30,  fanout: 8.0}         # ...and a second fanout
    #   - {from_depth: 31,  fanout: 1.02}

    # How shared CONTENT is replaced over time -- documents re-indexed, system
    # prompts redeployed, threads aging out of relevance. Each trunk node carries a
    # generation in its identity, and a node's generation advances on this half-life,
    # drawn from the node's own identity so it stays deterministic. Because the key is
    # a rolling hash, rotating a node automatically invalidates its whole subtree, so
    # this one number produces the full range of behaviours: the root rotating is a
    # whole-tree replacement, a mid-trunk node rotating replaces one branch's shared
    # content.
    #
    # 0 (the DEFAULT) means no churn: shared content, once minted, exists forever.
    # That is what every run before this parameter existed did.
    #
    # A depth-d path survives only while all d+1 of its nodes do, so the effective
    # half-life of a path is ~half_life/(d+1) -- shallow shared prefixes are stable and
    # deep ones are fragile, which is the way round real deployments behave.
    churn:
      half_life: 0            # e.g. 6h; 0 = never (default)

    # Which of those children a descending session picks: a Zipf exponent over child
    # rank. 0 = uniform; higher = a few children absorb most descents, giving a
    # heavy-tailed trunk-popularity distribution. Domain: >= 0 (0.9-1.2 is typical;
    # values above 1 are legitimate and common). May be given per segment inside
    # `branching` as `skew:`, which overrides this default for that segment; a fanout
    # into tool variants is often far more skewed than a fanout into user content.
    # A segment may likewise override `churn.half_life` as `churn_half_life:`, which is
    # how you say "prompts are stable for weeks, retrieved documents turn over daily".
    #
    # NOTE the interaction with `branching` (FR-055g): a fitted fanout sits near 1.0, so
    # a trunk node has one or two children, and a 2-way split is the commonest branch
    # point in real traces. Any skew must therefore leave BOTH children reachable —
    # until 2026-08-14 the sampler put probability 1 on the first at two children, which
    # made every non-zero `branch_skew` collapse the trunk to one path per root and made
    # `branching` inert in every document that did not say `branch_skew: 0`.
    branch_skew: 0.9
```

The session's private path length lives in `workload.sessions`, not here, because it is a
property of the session rather than of the shared structure. There is no `depth` field
anywhere: path length is stated once, by the formula in § `workload`.

### Why this generates prefix trees compactly

`CacheKey` is an opaque `u64`; in the vLLM path it is a rolling hash over the block chain.
So a shared prefix *is* a shared sequence of leading `u64`s, and the key space is a trie whose
node identity is the hash of its path. Key identity is therefore
`child_id = H(parent_id, child_index)` — the trie is never stored, and resident memory is
O(active paths) regardless of how many distinct keys a run mints.

**Sharing is necessarily a monotone prefix property, and that is what makes the model this
small.** Because the key is a rolling hash over the chain, divergence is *irreversible*: once
two paths differ at depth *d*, every key below *d* differs too, whatever the content. Two
requests can therefore share a prefix of some length and nothing else — they can never
re-converge. So the only free quantity per session is **where sharing ends**, which is exactly
`shared_depth`. A per-depth sharing table would offer degrees of freedom the key model cannot
realise (it could ask for "tight at depth 0, loose at 8, tight again at 16", which no
assignment of keys can satisfy), so this schema does not offer one.

### Trunk children are minted deterministically, not by arrival order

A node's children are decided **once, from the node's own identity and the seed**, by drawing the
fanout that `branching` gives for that node's depth. A descending session then chooses among those
children by `branch_skew`. It never invents one. Depth-indexing the fanout keeps this property
intact: depth is a property of the node, so the child count still depends only on the node's
identity and the seed, never on who arrived first.

The alternative — a Chinese-restaurant rule where a session either descends into a previously
visited child or mints a fresh one, which is what Pitman–Yor's discount did — is rejected for
three reasons, and they are the same reason wearing different hats:

- It makes the trie a function of **arrival order**, so `corpus` would no longer be orthogonal
  to `workload` (design rule 1): changing the request rate would change which keys exist.
- A key's identity would no longer be computable from its path alone; reconstructing it would
  require replaying every prior session in order, which defeats `child_id = H(parent_id,
  child_index)` and the O(active paths) memory bound.
- Per-node plan generation and plan verification both depend on any key being derivable from
  the seed without history.

**Private descents use a disjoint child namespace.** Below `shared_depth` a session walks
`child_id = H(parent_id, PRIVATE_TAG, session_id, i)`, so two sessions can never collide on a
private node. Without the tag, "private" would be only probabilistically private, and the
inter/intra separation that design rule 3 rests on would leak in a way no test would reliably
catch.

A handful of numbers then reproduce the whole practical family:

| Shape | Configuration | Models |
| --- | --- | --- |
| One global preamble | `roots.count: 1`, `shared_depth` median high | A single system prompt every request shares |
| A few task families | `roots.count` 5–20, `roots.popularity` skewed | Distinct prompt templates or tenants, unevenly used |
| Broad shallow sharing | `shared_depth` median low, `private_depth` median high | Requests agree on a short preamble then go their own way |
| Essentially no sharing | `roots.count` large, `shared_depth` → 0 | Scan / long-document ingest; every key novel |

| A shared tool or document per branch | `branching` with a fanout, then a flat segment, then another fanout | A global preamble, then per-branch commonality (a tool definition, a retrieved document), then the private tail — see § Trunk width is piecewise |

`branch_skew` shapes the popularity of trunks *within* a root, which is what makes the
mid-depth trunk population heavy-tailed rather than uniform. Trunk **width** is still not
configured directly: `w(0) = roots.count` and `w(d+1) = w(d) × fanout(d+1)`, so width remains
emergent from the profile, and the realised `w(d)` is reported per depth in the realised-corpus
summary rather than assumed. What the profile changes is that the fanout is no longer forced to be
the same at every depth — which is what lets a *flat* segment exist at all, and a flat segment is
what "everything on this branch shares this" means.

### Shared content churns on its own schedule, not on its readers'

`churn.half_life` exists because the trunk is otherwise **immortal**. Every trunk key is a pure
function of its path, so once minted it can be re-derived forever; on a long run every trunk key is
touched and then re-touched indefinitely, and all novelty comes from private branches. Real shared
content does not behave that way — documents are re-indexed, system prompts are redeployed, popular
threads stop being asked about — and because that discrepancy *grows* with run length rather than
staying constant, an arbitrarily long run gets progressively less representative without it.

The mechanism is a **generation term in node identity**:

```
child_id = H(parent_id, child_index, generation(node))
```

where `generation` advances on `churn.half_life`, drawn from the node's own identity and the seed.
Rotating a node changes its key, and because the key is a rolling hash **its entire subtree rotates
with it automatically** — which is why one parameter covers whole-tree replacement (the root
rotates), per-branch content replacement (a mid-trunk node rotates), and everything between.

The measurable event is a **compulsory-miss shock**: at the instant a shared node rotates, every
session that would have hit its old key now misses, all at once, and must re-populate. That is a
real and sharp phenomenon in a KV cache — a redeployed system prompt invalidates the most-shared
prefix in the system — and it is exactly the sort of transient that distinguishes replacement
policies from one another. Rotation events and the miss shock they cause are reported.

**Why not reference counting?** The obvious alternative is for sessions to hold refcounts on the
shared nodes they use and retire a node when the last user leaves. It does not work, for three
reasons that are worth recording because the idea is a natural one:

1. **It cannot create novelty.** Node identity is a pure function of the path, so a retired node is
   re-derived *identically* the moment another session walks the same child indices. Deletion hides
   a key briefly; it does not produce a new one. Only a generation term does.
2. **Refcount-zero fires in the wrong places.** For the § Worked example parameters there are ~833
   live sessions on each of the 12 roots but only ~1.1 per distinct path at depth 40. So refcounts
   essentially never reach zero near the root and reach it constantly deep down: the scheme would
   churn the nearly-private deep nodes and never touch the popular shallow ones. Real churn is
   driven by content lifecycle, which is uncorrelated with popularity — and the case that matters
   most hits the *top* of the trunk.
3. **It would couple `corpus` to `workload` again.** If a node exists only while some session holds
   it, then which keys exist depends on arrival timing, so changing the request rate would change
   the key space — the same objection that ruled out a Chinese-restaurant minting rule
   (§ Trunk children are minted deterministically). Generation-based churn keeps the key space a
   function of the seed and the clock, and of nothing about who is reading.

Reference counting *is* the right model for the part of the tree where the reader owns the content,
and that part is already handled: a session's private keys are dead the moment it retires
(§ Sessions are born and retired), with no refcount needed because the count is known to be one.

### Sharing is only realised if trunk paths are occupied

`shared_depth` is what a session *attempts*: it says "I leave the trunk at depth *s*." Whether
those *s* levels are actually **shared** depends on whether any earlier session walked the same
*s* steps. The drawn value is therefore an **upper bound** on realised sharing, and the quantity
that decides whether the bound is tight is **trunk occupancy** — how many sessions traverse each
distinct trunk path:

```
sessions_per_window = sessions begun within one window of run.wss_window REQUESTS
paths(d)            = roots.count * PRODUCT of fanout(k) for k in 1..d      # from `branching`
occupancy(d)        = sessions_per_window / paths(d)
```

Expressing `paths(d)` as a product over the profile rather than as `branch_factor^d` is what
makes occupancy computable for a piecewise trunk, and it is exact for the uniform case too, where
the product collapses back to `roots.count * branch_factor^d`.

**Churn shortens the window, and this is not optional bookkeeping.** A trunk path only accumulates
sharers for as long as it exists, so with `churn.half_life` set, the sessions that count toward
occupancy are those arriving within the path's *lifetime* rather than within the whole
`wss_window`:

```
path_lifetime(d)    = churn.half_life / (d + 1)      # all d+1 nodes must survive
effective_window(d) = min(wss_window, path_lifetime(d) expressed in requests)
occupancy(d)        = sessions arriving during effective_window(d) / paths(d)
```

Without this term the occupancy floor would pass a configuration whose sharing churn silently
destroys — the check would count a window's worth of sessions against a path that only lived for a
fraction of it. Note the interaction bites hardest exactly where sharing is deepest, since
`path_lifetime` falls as `1/(d+1)`: a `churn.half_life` generous at depth 4 can be far too short at
depth 40. This is the same failure shape as a warmup shorter than the session ramp — a
configuration that is internally consistent, passes every other check, and does not measure what it
claims to.

When `occupancy(s) ≫ 1`, every trunk path at depth *s* has been walked before, realised sharing
equals the drawn `shared_depth`, and § Fitting's one-pass measurements are exact. When
`occupancy(s) < 1`, sessions land on virgin trunk and realised sharing collapses far below the
drawn value — while the configuration still looks entirely reasonable.

**The window is part of the definition, not a refinement.** Occupancy counts sessions per
*eviction-relevant* window, because a block touched once a million requests ago has long since
been evicted. Counted over the whole run instead, a configuration could "achieve" sharing merely
by running longer, which is not a physical effect.

Worked, for the § Worked example parameters — 12 roots and ~40 000 sessions per 60 s window, so
~3 300 sessions per root:

| depth | trunk paths per root at `branch_factor: 1.25` | sessions per path | realised sharing |
| --- | --- | --- | --- |
| 4 | 2.4 | 1 400 | = drawn |
| 18 | 55 | 60 | = drawn |
| 40 | 7 500 | **0.4** | **≪ drawn** |

That example's `shared_depth` runs to depth 40, so at a uniform fanout of 1.25 the deepest-sharing
quartile — exactly the long-preamble and RAG cases the forest model exists to capture — would
silently fail to achieve its drawn sharing. Hence the `auto` default, which for a single uniform
segment solves

```
fanout = (sessions_per_window / roots.count / target_occupancy) ^ (1 / p99(shared_depth))
```

with **`target_occupancy = 4`**, giving ~1.18 for that configuration. This is a **closed form, not
an iterative calibration**: nothing in this schema requires a nonlinear fit.

`target_occupancy = 4` began as a judgement. It is now **corroborated by measurement**: across the
traces examined during design, occupancy below the fanout points settled in the **low single digits**
and held there across hundreds of depths. That sits just under the chosen target, which is the right
side to err on. The target remains a floor to design against, not an estimate of any population: the
traces examined were a sample of convenience and are not claimed to be representative.

**When sweeping `shared_depth`, pin `branching` explicitly.** `auto` re-solves at every sweep
point, which would vary the trunk shape along with the swept axis and confound the comparison.
Pin it to the profile valid at the deepest point of the sweep.

### Trunk width is piecewise, not smooth

The scalar this replaced assumed width grows as `branch_factor^depth` — smooth exponential
branching at every level. **Real tries do not look like that.** Width stays *exactly constant* for
long stretches and then jumps at particular depths. Measured on traces examined during design, and
given by character rather than by name because the files are not part of this repository and no
requirement rests on them:

| Trace character | Fanout events (>1.8× at one depth) | Longest run of *constant* width |
| --- | --- | --- |
| Agentic, tool-heavy | depth 1 **and depth 23** (2.1× each) | 40 depths at constant width |
| Agentic, long-context | depth 124 (2.1×) | 21 depths at constant width |
| Agentic, transactional | depth 110 (1.9×) | 16 depths at constant width |
| Production code assistant | depth 1 (31×) | plateau from depth 8 to 512 |
| Retrieval / RAG | depth 2 (four orders of magnitude) | plateau from depth 4 to 256 |

A constant width across 40 consecutive depths means **every node in that band has exactly one
child**. A uniform fanout of even 1.05 would widen by 7× over those 40 levels. So the shape a
scalar produces is not a coarse approximation of the real one; it is a different shape.

Two consequences, and the second is why the profile exists at all:

1. **A scalar fitted to a real trace comes out near 1.0 and means nothing.** It averages long flat
   runs against rare large jumps. The measured means were 1.009–1.078 for the agentic traces, and
   for chat and retrieval the same estimator gives 7.6–82, which is not a trunk width but an
   artifact of one enormous jump near the root.
2. **Fanout happens deep, so it cannot be folded into `roots.count`.** A single fanout event at
   depth 1 or 2 can be absorbed by redefining what counts as a root — a trace showing 155 roots that
   each split 31 ways is better described as ~4 900 roots. But fanout has been observed at **depth
   124**, after that many levels of genuinely shared path, and no choice of root boundary reaches
   that. Only a depth-indexed profile does.

The shape the profile buys is the one real traces actually have, and it is worth naming because it
is the interesting case for a cache: **a global prefix shared by everything, a fanout, then a
second shared segment on each branch** — a tool definition, a retrieved document, or a system
preamble common to that branch but not to the others — **and only then the private tail.** Two
sessions on the same branch share far more than the global prefix; two on different branches share
only it. An agentic tool-use trace showed exactly this, with fanouts at depths 1 and 23.

**This does not reopen the non-monotone-sharing question**, and the distinction is worth being
precise about because the two look similar. Divergence remains irreversible: once two sessions
take different children, every key below that point differs, forever. Sharing is still a *monotone
prefix* property of any *pair* of sessions, which is what the rolling hash requires and what
killed the old per-depth sharing table. What varies by depth here is only **how many children a
node has** — a property of the trie's shape, not of any pair's sharing — and that is realisable at
any profile, because a node having one child at depth 20 and forty at depth 21 contradicts
nothing. The earlier note that depth-varying branching "was unrealisable anyway" conflated the two;
only depth-varying *sharing* is unrealisable.

**What the profile still cannot express**: fanout depths that differ *between* branches — branch A
carrying a tool preamble that branch B lacks. The profile is global, so the trie is self-similar:
every branch fans out at the same depths. Modelling per-subtree structure would require a stage
table per branch, and the corpus does not yet show a case that demands it.

### Fitting from a real trace

Every structural parameter — the `corpus.trees` fields plus the session path lengths — is one
pass over a trace, which is the main reason this parameterisation was chosen over a
nonparametric branching process with no closed-form fit:

| Parameter | Measurement |
| --- | --- |
| `roots.count` | distinct keys at the **root boundary**, which is not always depth 0 — see below |
| `roots.popularity` | histogram of sessions per root |
| `shared_depth` | longest common prefix *within one `wss_window`* of each session's **turn-1 request only** — the space the parameter is drawn in, not the space it is validated in; see below |
| `branching` | the **width-by-depth profile** `w(d) = distinct keys at depth d that **two or more sessions** reached`, segmented per `research.md` § The branching segmentation rule; each segment's fanout is the geometric mean of the ratios inside it |
| `branch_skew` | Zipf exponent fitted to the visit-count distribution over the keys at one depth, per segment |
| `private_depth` | path depth of the **lowest-numbered turn** − that request's longest common prefix |
| `growth_per_turn` | path-depth increment between consecutive turns of one session, **in turn-index order**, accumulated **per session-length band** — see below |
| `churn.half_life` | **not fittable from a trace of ordinary length** — see below |

`shared_depth` is emitted as `empirical` because it **is** the FR-056 validation statistic's
quantity, so a parametric shape would be a worse model that looked more confident.

It is **not**, however, fitted over the validation statistic's population, and the distinction
costs a constant on every generated path if it is missed. `shared_depth` is drawn **once per
session** and every turn of that session then re-walks the same trunk prefix, so the parameter is a
per-session quantity while the statistic is a per-request one. The generator turns the first into
the second itself; fitting from the per-request histogram applies the turn weighting twice, and
because sessions that share more deeply also run longer, the doubled weighting biases it upward — by
+33.3, +74.6 and +238.1 blocks on every request of three agentic traces measured. `private_depth` is
`turn-1 depth − turn-1's own prefix`, so that bias lands on FR-014a's path in full.

**The two spaces cannot both be matched exactly, and the residual is a model limitation.** The
generated per-request histogram is the turn-weighted image of the per-session draw, so it equals the
trace's only where the trace's sharing is constant within a session. Real traces have two departures
from that, and `fit --explain` separates them because they call for different things: sessions that
share more deeply running longer is a **correlation** the model could express by conditioning
`shared_depth` on `turns`, while sharing that **deepens along the conversation** cannot be expressed
by any single per-session draw. Measured as KS distances, conditioning on `turns` would take the
sharing divergence from 0.0646 to 0.0488 on one trace and from 0.0954 to 0.0234 on another, and from
0.3409 only to 0.2311 on a third.

**Two of these are measured along the turn chain and the rest along the arrival stream, and the
difference is load-bearing.** A reader hands invocations over in timestamp order, which is correct
for `shared_depth` and for every reuse statistic — those are properties of a *stream*. It is wrong
for `growth_per_turn` and `private_depth`, which are properties of the *turn chain* of FR-014a, and
on real traces the two orders disagree: 14-17% of adjacent turn pairs in the agentic traces measured
arrive in an order their turn indices contradict.

Differencing the arrival sequence instead over-estimated the growth total by a measured **2.08x to
2.28x** on three of them. Each apparent decrease is clamped to zero while the positive increments on
either side of it are counted in full, so the sum exceeds the session's true span by twice the
decreases — and FR-014a then accumulates that excess into every later turn. It surfaced as synthetic
output 1.6x longer than its source and a `request_length` divergence of 0.18 against a 0.02
tolerance. In turn order those same traces have **zero** decreasing steps and an inflation factor of
exactly 1.000: they are perfect strict chains, and only their arrival order was disordered.
`private_depth` fails the same way for the same reason — the first *arrival* of a disordered session
is a mid-conversation request, so it puts a deeper path where turn one's belongs.

`think_time` stays on the arrival stream deliberately. It is a wall-clock gap between one session's
consecutive requests, so the stream is the axis that reproduces it, and in arrival order the gap is
non-negative by construction. Differencing timestamps along the chain would yield negative gaps on
16-17% of adjacent pairs of those traces, carrying 90% of the total positive magnitude; clamping
those would trade one silent bias for another.

A fit MUST therefore report arrival-order disorder and genuine chain violations **separately**. They
are different findings: the first says the trace's timestamps and turn indices disagree, which no
longer affects any fitted parameter, while the second says path depth decreases along the chain,
which FR-014a forbids and the model cannot express.

**`growth_per_turn` is fitted per session-length band, and a document may state it that way.**
A session's accumulated depth is `Σᵢ (T − i)·gᵢ` — an increment is inherited by every later turn, so
it enters with weight `T − i` and that weight is **quadratic in the turn count** once summed over the
session. One pooled distribution is therefore only right if the growth rate does not vary with session
length, and in real agentic traces it varies a great deal and **non-monotonically**: the rate climbs
from about 21 blocks/turn at 2–3 turns to 37–38 around 8–16 turns, then falls away sharply beyond 96.
A conversation that runs very long is one that grows slowly, which is what lets it run long. Ignoring
it made the accumulated depth come out 1.478x and 1.545x what the traces have; banding brings the same
arithmetic to ~1.00x (spec FR-054f).

So the table is accumulated **per rung of a geometric ladder** of turn counts
(`2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64, 96, 128`) — geometric because turn counts span orders of
magnitude and a linear grid would put nearly every session in the first cell — and rungs are then
**merged upward until a band carries at least 25 sessions**, with a short final band folded into its
neighbour. The emitted ladder is thus as fine as the data supports rather than as fine as the ladder,
and a trace that supports only one band emits the bare distribution, since a one-row table would
assert a length dependence the trace has not shown.

Both spellings are the same field. The bare form is unchanged and remains the default; the banded form
replaces the distribution with a `by_turns` list, and the applicable band is the last one whose
`from_turns` does not exceed the session's turn count:

```yaml
version: 1
seed: 1
duration: 60s
corpus:
  block_bytes: 128KiB
  trees:
    roots: {count: 12, popularity: {dist: zipf, s: 0.9}}
    shared_depth: {dist: lognormal, median: 12, sigma: 0.6}
workload:
  arrival: {model: open_loop, rate: 400/s}
  sessions:
    turns: {dist: geometric, mean: 6}
    think_time: {dist: const, value: 2s}
    private_depth: {dist: lognormal, median: 8, sigma: 0.8}
    # Banded by session length. Short conversations grow fastest; the very long
    # ones grow slowly, which is what lets them run long. The band is resolved
    # ONCE per session from its drawn turn count, then growth is drawn per turn
    # from that band -- a session's length selects the distribution, not the value.
    growth_per_turn:
      by_turns:
        - {from_turns: 1, growth: {dist: lognormal, median: 21, sigma: 0.7}}
        - {from_turns: 8, growth: {dist: lognormal, median: 37, sigma: 0.7}}
        - {from_turns: 32, growth: {dist: lognormal, median: 15, sigma: 0.7}}
run:
  mode: hardware
```

The first band MUST start at `from_turns: 1` and the bands MUST ascend, or the document is rejected by
rule 24 — a table that starts higher, or that does not ascend, routes some sessions to the wrong band
and every path they generate is then wrong with nothing later attributing it back here.

**`branching` counts only the shared keys, and this is load-bearing.** An earlier version of this
table defined `w(d)` as every distinct key at a depth, on the grounds that a trace cannot tell a
shared node from a private one. It can, wherever it has session identity: a node two sessions reached
is trunk, and a node one session reached is a private descent. Counting both counts the trunk *plus
every private path*, and for a workload with deep private descents that is wrong by orders of
magnitude — fitting the generator's own emitted trace that way recovered `roots.count: 1770` against
the 12 the source document stated, and the resulting model failed the FR-009f occupancy floor, so
`fit` refused to emit it. Counting shared keys only recovers 11 of the 12.

The consequence is that `branching` is fittable **only from a trace with session identity**, which is
also what makes the occupancy figure FR-055b requires reportable at all. A trace without it can still
supply arrival and size parameters; `fit` refuses the trunk rather than reading private width as
shared.

`fit` emits the **measured** `branching` profile, not `auto` — trunk structure is a property of the
trace — and records what `auto` would have chosen beside it. If the measured combination violates
the occupancy floor, `fit` fails rather than substituting (spec FR-055a): a combination the
generator cannot realise is exactly what FR-057 exists to refuse.

**The root boundary is chosen at the first fanout, not at depth 0.** Real traces begin with a
handful of keys — sometimes exactly one — shared by nearly every request, and then fan out sharply
within the first level or two. Taking `roots.count` literally as the depth-0 count would report
`roots.count: 1` for a trace whose every request shares one preamble, and would then have to express
the fanout immediately below — observed at up to four orders of magnitude in a single level — as
trunk branching, where it would fail the occupancy floor at any useful depth. So `fit` MUST place the root boundary at the depth **below the
last near-root fanout event**, report `roots.count` as the width there, and treat the levels above
it as a global preamble prepended to every session. A trace measuring 155-roots-with-a-31×-split is
then ~4 900 roots, and the profile below the boundary is flat, which is both true and realisable. The chosen boundary depth MUST appear in the fit report, since it changes what
`roots.count` means. This rule is only about *near-root* fanout; a fanout deep in the trunk (
observed beyond depth 100) is a genuine `branching` segment and MUST NOT be absorbed this way.

**`churn.half_life` MUST be left unset rather than estimated.** Its observable signature is a trunk
key that is used and then never used again for the rest of the trace — but every trace in the
trace of ordinary length spans hours at most, while plausible real churn periods run to days or
weeks. A
half-life longer than the observation window is indistinguishable from no churn at all, so any
fitted value would be an artifact of trace length rather than a property of the workload, and it
would be biased *short* — the direction that manufactures cache misses. What `fit` MAY legitimately
report is a **lower bound**: "no trunk rotation observed within the trace's N-hour span, so
`half_life` ≫ N". Setting churn is then a deliberate act by whoever knows the deployment's real
content cadence, which is not something a trace of this length can tell them.

**A measured fanout is only trustworthy where occupancy is high.** A trace reveals only
*visited* nodes. Where many sessions traverse each trunk path, most children are observed and the
measured width ratio approaches the true value; where occupancy is low, each session sits alone on
its own path and the ratio collapses toward **1 whatever the true branching**. So the fit report
states the occupancy at which each ratio was measured, and a ratio near 1 from a low-occupancy
region means "not observable here", not "the trunk is linear".

## `workload` — who asks for what, when

```yaml
workload:
  arrival:
    model: open_loop            # open_loop (default) | closed_loop
    rate: 4000/s                # open_loop only; distribution-valued
    burstiness: 1.8             # index of dispersion; 1.0 == Poisson (neutral value)
    concurrency: 256            # closed_loop only: bounded in-flight sessions

  # The SESSION is the only behavioural unit. A session binds to one root at birth
  # (see corpus.trees.roots.popularity) and every one of its turns starts from that
  # same root, which is what produces many sessions : one root. Defaults for the
  # whole population; `mix` entries override individual fields.
  sessions:
    turns: {dist: geometric, mean: 6}          # 1 == a one-shot request
    think_time: {dist: lognormal, median: 3s, sigma: 1.1}

    # Depth walked BELOW corpus.trees.shared_depth at TURN 1, on a branch private to
    # this session. These keys are reused only within the session, by later turns,
    # never across sessions.
    private_depth: {dist: lognormal, median: 8, sigma: 0.8}

    # Blocks added by each turn after the first. Drawn once PER TURN, not per session.
    # May instead be a table BANDED BY SESSION LENGTH -- see below.
    growth_per_turn: {dist: lognormal, median: 6, sigma: 0.5}

    # AGENT FAN-OUT. A session may spawn children that inherit its context and run
    # elsewhere -- the shape of an agent delegating sub-tasks to other nodes/GPUs.
    # This is the archetypal remote-lookup workload: the inherited prefix was minted
    # by the parent and is resident on the PARENT's node, while the children asking
    # for it are somewhere else, all at once. `self_affinity` cannot produce this --
    # a per-request coin flip gives the right average remote fraction with the wrong
    # structure, because a fan-out is correlated in BOTH node and time.
    # Disabled by default (`fanout: 0`): measured traces show diffuse, template-like
    # sharing rather than fan-out bursts, so a burst pattern must be asked for
    # explicitly and never assumed (see spec FR-018e).
    spawn:
      fanout: 0                                  # children per spawning session; 0 = off
      probability: 0.0                           # fraction of sessions that spawn
      at_turn: {dist: geometric, mean: 2}        # which turn triggers the spawn
      depth: inherit_all                         # inherit_all | {dist: ...} prefix depth
      generations: 1                             # 1 = children do not themselves spawn
      placement: other_nodes                     # other_nodes (default) | any | same_node

  # Weighted mixture over the SAME session model — each entry is a parameter set,
  # not a distinct code path. Weights are normalised, not required to sum to 1.
  # Any `sessions` field may be overridden. No `corpus` field may be: a mixture entry
  # varies how a session behaves, never what is shared (design rule 3).
  mix:
    - {weight: 0.70, turns: {dist: geometric, mean: 6}}     # "conversation"
    - {weight: 0.25, turns: 1}                              # "one_shot"
    - {weight: 0.05, turns: 1, private_depth: 4000}         # "scan"

  drift:
    half_life: 300s             # root-popularity non-stationarity; 0 (default) = stationary
```

Sessions are sticky in the root, not in the branch: turn N+1 re-reads turn N's blocks and
extends the path by `growth_per_turn`, so **intra**-session reuse comes from `turns` while
**inter**-session reuse comes from `corpus.trees`. Those are the two mechanisms of design
rule 3, and neither is expressible in the other's section.

### Agent fan-out, and the two invariants it bends

A spawn is the one place where **one session reads another session's private keys**, and that is
exactly why it generates remote traffic: the inherited prefix was *minted* by the parent, so it lives
on the parent's node, while the children asking for it are elsewhere and arrive together.

Two existing invariants have to be stated more precisely for this to be coherent, and neither is
weakened:

1. **Private namespaces are per-*minter*, not per-reader.** A private key is
   `H(parent_id, PRIVATE_TAG, minting_session_id, i)`, so a spawned child's inherited prefix keeps the
   **parent's** id in the tag, and the child mints in its own namespace only below the spawn point.
   Two sessions still cannot *collide*, because minting is still per-session; what a declared lineage
   adds is the ability to *read* along it. Without this the child would compute different keys for the
   parent's context, silently turning a fan-out into a cache miss storm and measuring nothing.
2. **A parent's private keys die when the parent *and all its descendants* have retired** — a
   lineage-scoped lifetime rather than a session-scoped one. Otherwise a parent that finishes while
   its children are still working would take their context with it.

That second rule is reference counting, and it is worth naming as such: refcounting was rejected for
the shared trunk (§ Shared content churns on its own schedule) because content lifecycle there is
independent of readers and refcount-zero never fires near the root. Within a lineage the relationship
is the opposite — the children *are* the readers, the parent's context exists precisely for them, and
the count is small and known. Same mechanism, right scope.

**`spawn` and `self_affinity` are independent and both may be set**, but the report MUST then
attribute remote traffic to each separately: one is structural and bursty, the other is a smooth
per-request probability, and a single aggregate remote fraction would hide which mechanism produced
it.

### Sessions are born and retired, so a run has no natural end

A session is born on arrival, binds its root, issues `turns` requests separated by `think_time`,
and is **retired** when its last turn completes; its private keys are dead from that moment and
are never read again by anyone. So the generator runs for as long as you ask by continuously
retiring old sessions and creating new ones, and the distinct-key count grows without bound
because every new session mints a fresh private branch.

**There is deliberately no `lifetime` field.** Lifetime is `Σ think_time` over the session's turns
— already determined by `turns` and `think_time` — and a third statement of it could disagree with
them (design rule 3). For the same reason there is no field for how many sessions are live at
once: under `open_loop` that follows from Little's law,

```
session_rate  = arrival.rate / mean(turns)          # 4000/s / 6   = ~667 sessions/s
mean_lifetime = (mean(turns) - 1) * mean(think_time) # 5 * 3s      = 15 s
live_sessions = session_rate * mean_lifetime         #             = ~10 000
```

and under `closed_loop` it *is* `arrival.concurrency`, which is legitimate there because
`closed_loop` supplies no rate. Both are reported rather than configured.

Two consequences that are easy to miss:

- **`run.warmup` must cover the ramp-up, not just the cache.** At t=0 no session is live and the
  population fills over roughly one `mean_lifetime`. A window that opens sooner sees fewer
  concurrent sessions and therefore less sharing than the model asks for. 20 s of warmup covers the
  15 s above, but `turns: geometric(50)` with `think_time` median 30 s implies a **~24 minute**
  ramp. The generator rejects a warmup shorter than the computed ramp (spec FR-015b).
- **Shared content turns over only if you ask it to.** `drift` changes which roots are *popular*; it
  does not retire trunk keys or mint new ones. Set `corpus.trees.churn.half_life` for that
  (§ Shared content churns on its own schedule) — at the default of 0 the trunk is immortal, every
  trunk key stays live forever, and all novelty comes from private branches. That default is fine
  for a run short relative to the deployment's real content cadence and progressively wrong for a
  long one, since the discrepancy *grows* with run length rather than staying constant.

### Path length, stated once

Turn N's path is a strict prefix of turn N+1's — the rolling hash requires it, since a changed
prefix would rehash every block below it — so depth grows monotonically across a session:

```
depth(turn N) = shared_depth + private_depth + Σ(i = 2..N) growth_per_turn(i)
```

`private_depth` is the turn-1 private path and `growth_per_turn` is the per-turn increment;
they measure different things, which is why both can exist without restating each other. A
session with the defaults above ends at roughly `18 + 8 + 5×6 = 56` blocks, or ~7 MB at
128 KiB. With `turns: 1` the sum is empty and depth is just `shared_depth + private_depth`.

### `open_loop` vs `closed_loop` — this choice affects validity

Under `closed_loop`, arrival times depend on how fast the system responds, so **two
replacement policies see different key streams** and any hit-rate comparison between them is
confounded by the system's own speed. Use:

- **`open_loop`** (default) for hit-rate and policy comparison — absolute timestamps keep the
  key stream identical across arms. The runner reports cumulative schedule lag, and will not
  claim a configured offered rate was achieved when the schedule slipped.
- **`closed_loop`** for throughput and saturation measurement, where queueing is the
  phenomenon of interest.

### The three familiar archetypes are presets, not schema

`conversation`, `one_shot`, and `scan` are the names of three points in the session
parameter space. They are shipped as presets and are useful vocabulary in reports, but they
are **not** distinguishable modes in the schema, and there is no `archetype:` field:

| Name | Parameter set | Models |
| --- | --- | --- |
| `conversation` | `turns: {dist: geometric, mean: 6}` | Multi-turn chat. Recency-friendly — the dominant real KV-cache pattern |
| `one_shot` | `turns: 1` | Independent requests over a shared trunk. Frequency-friendly |
| `scan` | `turns: 1, private_depth: 4000` | Long-document ingest. The classic LRU-polluting case |

Collapsing them removes three bespoke fields that each restated something `corpus` already
says: `popularity` (now `roots.popularity`), `novel_fraction` (now implied by
`private_depth` ≫ `shared_depth`), and `length_blocks` (now `private_depth`). One model means
one sampling path and one fitting routine.

There is no top-level `gets_per_key` or `lifetime`, and no per-entry `popularity`; supplying
either is a schema error (spec FR-007).

## `topology` — which node asks for what

```yaml
topology:
  nodes: [node2, node7, node9, node11]

  # How a SESSION maps onto nodes. Default `sticky`: a session binds to one node at
  # birth, exactly as it binds to one root, because a session's KV lives wherever it
  # was computed. Under `per_request` each request is placed independently, which
  # makes a session remotely fetch its OWN earlier turns -- occasionally what you
  # want to measure, never what a deployment does, and it drowns the cross-session
  # signal. Sessions are still distributed uniformly over nodes in aggregate, and
  # there are still no requester/holder roles.
  placement: sticky           # sticky (default) | per_request

  # Probability that the node asking for a key is one of the nodes that earlier
  # asked for it -- i.e. how much the per-node request streams overlap.
  # 1.0 = each node walks its own keys and no key is ever shared across nodes;
  # 0.0 = a key is always asked for by a node other than the ones that saw it.
  # This is the STRUCTURE-FREE dial: it hits a target remote fraction without
  # saying why. For structured remote traffic see workload.sessions.spawn.
  self_affinity: 0.25

  # How many distinct nodes ask for the same key, across the whole run.
  replication:
    nodes_per_key: {dist: const, value: 1}

  # Fraction of keys that the warmup phase deliberately does not pre-request,
  # so their first appearance in the measured window is their first appearance
  # anywhere.
  cold_fraction: 0.05

  membership_events:
    - {at: 60s, action: stop,  node: node9}
    - {at: 90s, action: start, node: node9}
```

**Request placement is uniform across nodes and there are no requester or holder roles.**
Every node both requests and holds. Real cross-node KV copies are essentially symmetrical, so
role assignment would model the lab rather than the deployment. Hardware asymmetry is handled
by `preflight` refusing to run (spec FR-049..FR-053), not by steering load.

## There is no `system` section

Earlier drafts carried one — capacities per tier, an eviction policy, watermarks mapping onto
`DispatcherConfig`, a pinned fraction. It is gone, along with `topology.holder_tier`, under
design rule 6: the generator has no business knowing what tiers the consumer has, or whether it
has tiers at all. Where those quantities went:

| Was | Now |
| --- | --- |
| `system.capacity.dram` / `.ssd` | the consumer's own configuration — a server's config file, or a simulator's `--capacity` |
| `system.eviction_policy` | the consumer's choice, expressed on the consumer's own command line |
| `system.thresholds` | server-side configuration only; never expressed here |
| `system.pin_fraction` | not a workload property at all; whoever operates the cache decides what it pins |
| `topology.holder_tier` | removed with no replacement — see § No tier placement |

What replaces the useful part of `fraction_of_wss` is a *published statistic* rather than an
input: the generator reports the **realised working-set size** over `run.wss_window`, so a
consumer that wants a cache at a quarter of the working set computes that itself. This keeps the
same ergonomics for capacity sweeps — the sweep is over the consumer's capacity flag, and the
working set it is a fraction *of* is still a single number in the report — without the workload
document ever naming a cache.

### No tier placement

`holder_tier` used to force the authoritative copy onto DRAM or SSD by driving `FlushToSsd` and
`ClearMemoryTier` during plan setup. Both the field and that setup step are removed. A workload
cannot state where a copy lives, because in general it does not know that anything *is* a copy,
or that there is more than one place for one to be. Where a block is resolved from is an
*outcome* the consumer reports, not an input the workload supplies — so a run that wants to
exercise a slower medium gets there by asking for more distinct bytes than the faster medium
holds, which is a statement about the workload, and reads the resulting split out of the
consumer's own reporting.

## `run` — execution and measurement

```yaml
run:
  mode: hardware              # where the requests go; see the note below on output modes
  endpoint_template: "{node}:50051"
  batch_size: 64              # keys per RPC
  workers: 8                  # client threads
  inflight: 4                 # concurrent RPCs per worker
  gpu_buffer: 8GiB            # one process-wide CUDA allocation, addressed by offset
  # Excluded from steady-state statistics, and it MUST cover the session-population
  # ramp (rule 15b): (mean(turns) - 1) x mean(think_time) = 5 x 5.5s = 27.5s here, the
  # lognormal's mean being median x exp(sigma^2/2) = 3 x 1.83 rather than its median.
  # 20s would be rejected -- the measured window would open mid-ramp.
  warmup: 30s
  warm_connections: true      # explicit RDMA connection-warm phase before measuring
  # Window for the working-set-size calculation AND for trunk occupancy. Canonically a
  # REQUEST COUNT, because the plan is a sequence and a count is knowable at plan time in
  # both arrival modes. A duration is accepted as sugar and converted via the configured
  # rate, which only open_loop has -- a duration under closed_loop is a schema error.
  wss_window: 240_000         # or `60s` under open_loop at 4000/s (identical)
  clock_skew_bound: 1ms       # preflight fails above this
  emit_trace: /tmp/plan.jsonl # optional, debugging only; never an input
```

`mode` names **where the generated requests go**. The set of modes is being reworked: the
intended set is a live Certus server, a `.jsonl` trace file, and a parquet trace file, of which
only the first involves Certus at all. The earlier `simulate` mode is gone with cache simulation
(spec Out of Scope). Until the trace-file formats are pinned down against real examples, treat
`hardware` as the only defined value.

`wss_window` is a **request count**, not a wall-clock span, so that the working-set size and the
trunk occupancy it feeds are determined by the plan rather than by how fast the consumer happened
to run it. Under `closed_loop` a time window would be unknowable at plan time (arrivals depend on
the consumer's response, and `t_ns` is advisory ordering only), and under `open_loop` it would
drift whenever the schedule slips, which FR-061 exists to report. A count is exact in both cases.
The realised working-set size over the window is recorded in the plan summary, which is what
makes it usable by a consumer sizing a cache (design rule 6) without the workload naming one.

## `sweep` — the experiment matrix

```yaml
sweep:
  axes:
    topology.self_affinity: [0.0, 0.25, 0.5, 1.0]
    corpus.trees.shared_depth.median: [4, 8, 16, 32]
  repeat: 8                   # default 8
  order: interleaved          # interleaved (default) | blocked
```

Axes form a cartesian product. Dotted paths address any scalar in the document. Each
`(point, repeat)` gets a seed derived deterministically from the root `seed`, so an entire
sweep is reproducible from one number.

**`sweep` sweeps the workload only.** Every axis is a dotted path into this document, so a
consumer-side quantity — a cache capacity, an eviction policy — is not addressable here and
never was a legitimate axis (design rule 6). Those sweeps belong to the consumer, and are
already expressed that way: capacity and policy are command-line options of whatever grades
them, so a capacity sweep is a loop over that flag against one fixed plan. That division is the
stronger arrangement for a further reason — a workload axis changes the key stream and so needs
a fresh plan per point, while a consumer axis must hold the key stream *identical* across points
for its comparison to mean anything (FR-036). Keeping them in different places makes it hard to
accidentally vary both at once, which is the mistake that silently invalidates a comparison.

`repeat` defaults to **8** because prior measurement on this bench established that n = 3
produced misleading conclusions and n ≥ 8 is needed for significance. `order: interleaved`
rotates through points across repeats rather than completing all repeats of one point first,
so slow environmental drift does not alias onto a single sweep point.

## Presets and `extends`

`extends` deep-merges a base document; the including document wins on every conflicting leaf.
Lists replace rather than append. This is what delivers the compactness target: a common
experiment should be under ten lines.

```yaml
extends: presets/conversational-multinode.yaml
seed: 7
sweep:
  axes: {topology.self_affinity: [0.0, 0.25, 0.5, 1.0]}
  repeat: 8
```

Presets to ship, one per Test Matrix family:

| Preset | Shape |
| --- | --- |
| `presets/zipf-baseline.yaml` | `shared_depth: 0`, `private_depth: 1`, Zipf over many roots — a flat key space. Harness validation: LRU hit rate is analytic |
| `presets/conversational.yaml` | `turns` geometric 1.0 weight. Recency-friendly |
| `presets/shared-preamble.yaml` | `turns: 1`, `roots.count: 1`, `shared_depth` median high. Frequency-friendly |
| `presets/mixed.yaml` | The mixture, set up for a weight sweep. The headline experiment |
| `presets/scan-pollution.yaml` | Hot conversational set plus 5% at `turns: 1, private_depth: 4000` |
| `presets/conversational-multinode.yaml` | `presets/conversational.yaml` plus a 4-node `topology` |
| `presets/cold-storm.yaml` | High `cold_fraction`, for the cost of keys nothing has seen before |
| `presets/fitted-sharegpt.yaml` | Emitted by `fit` against the checked-in ShareGPT trace |

## Validation rules

The generator rejects, rather than silently accepting:

1. Unknown fields anywhere in the document.
2. A `version` it does not implement.
3. Either kind of reuse specified outside its own section (FR-007) — a `gets_per_key` or
   `lifetime` anywhere, a `depth` field anywhere, or any `corpus` field inside
   `workload.mix`, `popularity` and `shared_depth` in particular.
4. Any populate/put specification (FR-023).
5. Both `duration` and `requests`, or neither.
6. A `mix` with no entries, or all weights zero.
7. Distribution parameters outside their domain (`zipf.s <= 0`, `sigma < 0`, negative sizes,
   fractions outside `[0, 1]`).
8. `roots.count < 1`, `branch_skew < 0`, any `branching` segment with `fanout < 1` (a trunk node
   with no children
   would let a session run off the end of the trunk), or an `n` supplied to
   `roots.popularity` (its support is `roots.count`). Also rejected: a **bounded-support**
   `roots.popularity` whose support does not span `1..=roots.count` — an `empirical` whose top rank
   falls short leaves every rank above it unreachable, so the realised root layer is narrower than
   the document declares, and it is silent because a draw inside a narrow support records no clamp.
9. A corpus that mints no keys below the trunk — `sessions.private_depth` const 0 with
   `roots.count` and `shared_depth` also fixed makes the key space finite, so no eviction is
   ever exercised and the run is meaningless. Also rejected: `empirical` `shared_depth` or
   `roots.popularity` points not in **non-decreasing** value order, with a decreasing cumulative
   probability, or with a final cumulative probability ≠ 1.0. Non-decreasing rather than strictly
   ascending because the step encoding `fit` emits repeats each value on purpose — `(v, c_before),
   (v, c_after)` is how a discrete distribution passes through an interpolating reader — so a check
   demanding strict ascent would reject every fitted document. A CDF that stops below 1.0 makes every
   draw above it return the top point, silently collapsing that mass onto one value.
10. `replication.nodes_per_key` exceeding `len(topology.nodes)`.
11. `topology.membership_events` referencing a node not in `topology.nodes`, or an `at` beyond
    `duration`.
12. `mode: hardware` with no `topology.nodes` and no endpoint.
13. Any of the removed consumer-side keys — a `system:` section at top level, or
    `topology.holder_tier` — which rule 5 already rejects as unknown, but which MUST be
    rejected with a message naming design rule 6 and saying where the quantity now lives,
    because these were documented schema in an earlier draft and a stale document is a likely
    input rather than a typo.
14. `sweep.axes` dotted paths that do not resolve to a scalar in the document. Note that this
    is what now rejects a capacity or policy axis: there is no path for it to resolve to.
15. A `run.wss_window` expressed as a **duration** together with `arrival.model: closed_loop` —
    the conversion to a request count needs a configured rate, which only `open_loop` has.
    State the window as a request count instead.
16. **`occupancy(p99(shared_depth)) < 1.0`** — the trunk is wider than the session population
    can occupy, so sessions land on virgin trunk and realised sharing is far below the drawn
    `shared_depth` (§ Sharing is only realised if trunk paths are occupied). Occupancy below
    4.0 is a warning rather than a rejection, and the realised value is always reported. This
    is the one rule that catches a configuration which is internally consistent, passes every
    other check, and still does not measure what it claims to. When `churn.half_life` is set, the
    occupancy this rule tests MUST be the churn-adjusted one — sessions arriving within the
    *path's* lifetime, not within the whole window — since otherwise churn destroys sharing that
    the check has already approved.
17. `churn.half_life < 0`, or a `churn.half_life` shorter than `run.warmup`. A shared structure
    that turns over faster than the warmup takes to fill means the measured window opens on a
    trunk with no history at any depth, so nothing that follows describes the configured sharing.
18. `churn.half_life` set together with `mode` writing a trace file, **unless** `duration` is also
    set. Churn is a function of elapsed plan time, so a plan of a fixed *request count* or *block
    count* with no duration has no clock against which a half-life means anything.
19. More or fewer than one of `duration` | `requests` | `blocks` | `unbounded`.
20. A **file** output mode without `blocks`. A file's size is a block count; `duration` and
    `requests` both leave it at the mercy of the drawn request-length distribution, and an
    overlong run fills the filesystem (spec FR-021d).
21. `unbounded: true` with a **file** output mode (spec FR-021e). Unbounded is meaningful only when
    nothing accumulates — that is, when driving a server directly.
22. `spawn.fanout > 0` with fewer than two `topology.nodes` under `placement: other_nodes`, since
    there is nowhere else for a child to go. Also rejected: `spawn.generations < 1`, a
    `spawn.probability` outside `[0, 1]`, and `spawn.fanout > 0` with `spawn.probability: 0` or the
    reverse — a half-configured fan-out that silently does nothing.
23. `spawn.fanout > 0` together with `topology.placement: per_request`. Per-request placement already
    scatters a session across nodes, so the fan-out's defining property — an inherited prefix resident
    on one specific node — does not hold, and the measurement would attribute to fan-out what
    placement caused.

## Worked example — the headline mixture experiment

Complete and runnable; 50 lines including comments.

```yaml
version: 1
seed: 0xC0FFEE
duration: 180s

corpus:
  block_bytes: 128KiB
  trees:
    roots:
      count: 12
      popularity: {dist: zipf, s: 0.9}
    shared_depth: {dist: empirical, points: [[4, 0.10], [18, 0.75], [40, 1.0]]}
    branching: auto            # resolves to a uniform ~1.18 here; 1.25 would starve depth 40
    branch_skew: 0.9

workload:
  arrival: {model: open_loop, rate: 4000/s, burstiness: 1.8}
  sessions:
    turns: {dist: geometric, mean: 6}
    think_time: {dist: lognormal, median: 3s, sigma: 1.1}
    private_depth: {dist: lognormal, median: 8, sigma: 0.8}
    growth_per_turn: {dist: lognormal, median: 6, sigma: 0.5}
  mix:
    - {weight: 0.70}                                   # conversation: the defaults above
    - {weight: 0.25, turns: 1}                         # one_shot
    - {weight: 0.05, turns: 1, private_depth: 4000}    # scan

topology:
  nodes: [node2, node7, node9, node11]
  self_affinity: 0.25
  replication: {nodes_per_key: 1}
  cold_fraction: 0.05

run:
  mode: hardware
  batch_size: 64
  workers: 8
  inflight: 4
  warmup: 30s                 # >= the 27.5s population ramp; see rule 15b

sweep:
  axes: {workload.mix.0.weight: [0.4, 0.55, 0.70, 0.85]}
  repeat: 8
```

The swept axis is the conversational share of the mixture — the headline experiment — because
that is a property of the workload. The capacity sweep this example previously carried is now a
loop over the consumer's own capacity option against this one plan, which is both where it
belongs and what keeps the key stream identical across capacity points.
