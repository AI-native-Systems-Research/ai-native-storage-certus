// Helpers for driving the REAL component through the REAL `IEvictionPolicy`.
//
// `EvictionPolicyOptimizedComponent::new_default()` is the production constructor:
// `define_component!` expands it to `Self::new(Default::default())`, which builds a
// `component_core::component::InterfaceMap` — and that is a
// `HashMap<TypeId, Box<dyn Any + Send + Sync>>`. Constructing a std `HashMap` calls
// `RandomState::new`, which reaches `getrandom` and then a raw `syscall`, which Kani
// cannot model. That is the documented, DEFEATABLE HashMap-construction wall, not a
// tool boundary: [`concrete_state`] below is the deterministic-seed stub the skill
// prescribes, applied as
//
//     #[kani::stub(std::collections::hash_map::RandomState::new, crate::api::concrete_state)]
//
// on every harness that builds a component. Soundness of the stub: it fixes the
// SipHash keys to `[0, 0]`, which is sound for map SEMANTICS (insert / get / len /
// containment) and therefore for `query_interface`-style lookup by `TypeId`, which is
// all `InterfaceMap` does. Nothing in these harnesses asserts anything that depends on
// hasher randomness (iteration order, DoS resistance), so the fixed seed cannot
// launder a false pass.
//
// Everything below the constructor is untouched production code: the real
// `RwLock<EvictionState>`, the real `Vec<Mutex<Pool>>`, the real `CountMinSketch`, the
// real `LruList`. Harnesses call the `IEvictionPolicy` methods directly on the concrete
// type rather than through `component_core::query_interface!` because this crate cannot
// depend on `component-core` directly (see Cargo.toml); the method bodies reached are
// identical either way, since `query_interface!` only downcasts an `Arc` to the same
// inherent trait impl.

use std::collections::hash_map::RandomState;
use std::mem::{size_of, size_of_val, transmute};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub use eviction_policy_optimized::EvictionPolicyOptimizedComponent;
pub use interfaces::{
    BlockSemantics, CacheKey, EvictionHandle, EvictionPolicyError, IEvictionPolicy, ILogger, PoolId,
};

/// Deterministic `RandomState`, for `#[kani::stub(RandomState::new, concrete_state)]`.
pub fn concrete_state() -> RandomState {
    let keys: [u64; 2] = [0, 0];
    // Guards the transmute against a libstd layout change: if `RandomState` ever stops
    // being two `u64`s this assertion fails loudly inside the proof instead of the stub
    // silently producing garbage.
    assert_eq!(size_of_val(&keys), size_of::<RandomState>());
    unsafe { transmute(keys) }
}

/// A fresh real component with no logger connected.
///
/// No logger is the interesting configuration for almost every property: the
/// production code guards every log site with `if let Ok(logger) = self.logger.get()`,
/// so with the receptacle unconnected the `format!` calls are never evaluated and the
/// harnesses stay cheap — and `EPO-INV-LOGGER-OPTIONAL` is precisely the claim that
/// this changes no result.
pub fn component() -> Arc<EvictionPolicyOptimizedComponent> {
    EvictionPolicyOptimizedComponent::new_default()
}

/// A real component with one pool already created, and that pool's id.
pub fn component_with_pool() -> (Arc<EvictionPolicyOptimizedComponent>, PoolId) {
    let c = component();
    let p = c.create_pool();
    (c, p)
}

/// `BlockSemantics` with a symbolic session id — the hint this policy is supposed to
/// ignore entirely (`EPO-TRACK-FRAME-SEMANTICS-IGNORED`).
pub fn any_semantics() -> BlockSemantics {
    BlockSemantics {
        session_id: kani::any(),
    }
}

/// Drain a pool's victims, at most `cap`, as the eviction order the pool would apply.
pub fn drain_order(
    c: &Arc<EvictionPolicyOptimizedComponent>,
    pool: PoolId,
    cap: usize,
) -> Vec<CacheKey> {
    let mut out = Vec::new();
    while out.len() < cap {
        match c.identify_next_to_evict(pool) {
            Some(k) => out.push(k),
            None => break,
        }
    }
    out
}

/// `BlockSemantics` with the session id every session-unaware caller passes.
///
/// Spelled out rather than `Default::default()` so the harnesses carry no hidden input:
/// the one property that is ABOUT this argument
/// (`verify_epo_track_frame_semantics_ignored`) makes it symbolic instead.
pub fn sem() -> BlockSemantics {
    BlockSemantics { session_id: 0 }
}

/// Element-by-element key-sequence equality.
///
/// NOT `==` on the slices. Slice equality lowers to `memcmp`, whose byte loop CBMC has to
/// unwind over the whole buffer, so comparing a three-key `Vec<CacheKey>` under this
/// file's small unwind bound fails with "unwinding assertion loop 0" in
/// `<builtin-library-memcmp>` — a bound artefact and not a property failure. The same
/// reason `inspect::key_seq_eq` exists on the list side.
pub fn keys_eq(a: &[CacheKey], b: &[CacheKey]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0usize;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// True iff `r` is exactly the invalid-pool error naming `pool` — and therefore NOT
/// `EvictionPolicyError::InvalidHandle`, which this component constructs nowhere
/// (`EPO-INV-INVALID-HANDLE-NEVER-RETURNED`).
pub fn is_invalid_pool<T>(r: &Result<T, EvictionPolicyError>, pool: PoolId) -> bool {
    match r {
        Err(EvictionPolicyError::InvalidPool(p)) => *p == pool,
        _ => false,
    }
}

/// A logger that counts the messages it is handed, at each level.
///
/// Needed by the three obligations that are ABOUT logging rather than about eviction:
/// `EPO-BATCH-TOUCH-ERROR-NO-LOG` (the batch error path must stay silent where its
/// single-handle siblings warn), `EPO-INV-LOGGER-OPTIONAL` and
/// `EPO-INV-NO-STARTUP-BANNER`. It records nothing but counts, so connecting it cannot
/// change any eviction result — which is what lets the same harness compare the connected
/// and unconnected runs.
pub struct CountingLogger {
    pub errors: AtomicUsize,
    pub warns: AtomicUsize,
    pub infos: AtomicUsize,
    pub debugs: AtomicUsize,
}

impl CountingLogger {
    pub fn new() -> Self {
        Self {
            errors: AtomicUsize::new(0),
            warns: AtomicUsize::new(0),
            infos: AtomicUsize::new(0),
            debugs: AtomicUsize::new(0),
        }
    }
    pub fn total(&self) -> usize {
        self.errors.load(Ordering::SeqCst)
            + self.warns.load(Ordering::SeqCst)
            + self.infos.load(Ordering::SeqCst)
            + self.debugs.load(Ordering::SeqCst)
    }
}

impl ILogger for CountingLogger {
    fn error(&self, _msg: &str) {
        self.errors.fetch_add(1, Ordering::SeqCst);
    }
    fn warn(&self, _msg: &str) {
        self.warns.fetch_add(1, Ordering::SeqCst);
    }
    fn info(&self, _msg: &str) {
        self.infos.fetch_add(1, Ordering::SeqCst);
    }
    fn debug(&self, _msg: &str) {
        self.debugs.fetch_add(1, Ordering::SeqCst);
    }
}

// ------------------------------------------------------------------ infallible wrappers
//
// `Result::unwrap` is deliberately NOT used in the harnesses. It goes through
// `core::panicking::panic_display` with `#[track_caller]`, and `caller_location` is on
// Kani's unsupported-construct list for this crate; a reachable one turns a proof into an
// "unsupported construct" failure that says nothing about the property. These wrappers
// assert success explicitly instead, so the obligation "this call must succeed" is a
// first-class check the scorer sees.

pub fn track_ok(
    c: &Arc<EvictionPolicyOptimizedComponent>,
    pool: PoolId,
    key: CacheKey,
) -> EvictionHandle {
    match c.track(pool, key, sem()) {
        Ok(h) => h,
        Err(_) => {
            assert!(false, "track was required to succeed here");
            EvictionHandle::new(pool, 0)
        }
    }
}

pub fn track_ok_sem(
    c: &Arc<EvictionPolicyOptimizedComponent>,
    pool: PoolId,
    key: CacheKey,
    s: BlockSemantics,
) -> EvictionHandle {
    match c.track(pool, key, s) {
        Ok(h) => h,
        Err(_) => {
            assert!(false, "track was required to succeed here");
            EvictionHandle::new(pool, 0)
        }
    }
}

pub fn touch_ok(c: &Arc<EvictionPolicyOptimizedComponent>, h: EvictionHandle) {
    assert!(c.touch(h).is_ok());
}

pub fn remove_ok(c: &Arc<EvictionPolicyOptimizedComponent>, h: EvictionHandle) {
    assert!(c.remove(h).is_ok());
}

pub fn batch_ok(c: &Arc<EvictionPolicyOptimizedComponent>, hs: &[EvictionHandle]) {
    assert!(c.batch_touch(hs).is_ok());
}

pub fn new_counting_logger() -> Arc<CountingLogger> {
    Arc::new(CountingLogger::new())
}

/// Connect a counting logger to a live component's `logger` receptacle.
///
/// The receptacle field is `pub` (generated by `define_component!`) and
/// `Receptacle::connect` is public, so this needs nothing the framework does not already
/// offer to a normal assembler.
///
/// WHEN TO CONNECT MATTERS, and the harnesses are written around it. Each of the four log
/// sites in `lib.rs` evaluates `format!` into a `String` before calling the logger
/// (lib.rs:99, :113, :147, :191). `format!` is a well-known CBMC cost: integer-to-decimal
/// formatting is a loop over digits inside `core::fmt`'s dynamic-dispatch machinery, and at
/// the unwind bounds these harnesses run at it dominates everything else. So a harness that
/// needs a logger connects it AFTER the calls that log — `create_pool` logs a `debug` on
/// every call — and then exercises only paths with no log site, which is exactly the set the
/// two logging obligations here are about. Where a log site is deliberately NOT covered, the
/// harness's doc comment says so.
pub fn connect_logger(c: &Arc<EvictionPolicyOptimizedComponent>, lg: &Arc<CountingLogger>) {
    let as_iface: Arc<dyn ILogger + Send + Sync> = lg.clone();
    assert!(c.logger.connect(as_iface).is_ok());
}

pub fn attach_logger(c: &Arc<EvictionPolicyOptimizedComponent>) -> Arc<CountingLogger> {
    let lg = new_counting_logger();
    connect_logger(c, &lg);
    lg
}
