//! Creusot spec skeleton for dispatcher verification.
//!
//! This file is intentionally a scaffold: it defines a ghost model, core
//! predicates, and contract stubs mapped to `SPECS/dispacher/first_properties.md`.
//! It is meant to be refined alongside the real implementation.
//!
//! Suggested integration:
//! 1. Add `creusot-contracts` / `creusot-std` as verifier-only deps.
//! 2. Gate this module behind `#[cfg(creusot)]` in `lib.rs`.
//! 3. Replace each `todo!()` with links to concrete state/fields.

#![allow(unused)]

use interfaces::{CacheKey, DispatcherError, DispatcherConfig, IpcHandle};

#[cfg(creusot)]
use creusot_std::prelude::*;

/// Bound from FR-024.
pub const MAX_EVICT_ATTEMPTS: usize = 512;

/// Logical entry state in the verification model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecEntryState {
    MemoryTier,
    BlockDevice,
    Staging,
    PendingWrite,
}

/// Ghost snapshot of dispatcher-relevant state.
///
/// This is an abstract model. Connect it to concrete fields via refinement
/// predicates once verifier integration starts.
#[derive(Default, Clone, Debug)]
pub struct SpecModel {
    pub initialized: bool,
    pub memory_used: usize,
    pub memory_capacity: usize,
}

#[cfg(creusot)]
#[logic]
pub fn wf(m: SpecModel) -> bool {
    pearlite! { m.memory_used@ <= m.memory_capacity@ }
}

#[cfg(creusot)]
#[logic]
pub fn contains_key(_m: SpecModel, _key: CacheKey) -> bool {
    // TODO: refine to dispatch-map membership.
    pearlite! { true }
}

#[cfg(creusot)]
#[logic]
pub fn state_of(_m: SpecModel, _key: CacheKey) -> SpecEntryState {
    // TODO: refine to per-key state in ghost map.
    SpecEntryState::MemoryTier
}

#[cfg(creusot)]
#[logic]
pub fn timestamp_of(_m: SpecModel, _key: CacheKey) -> Int {
    // TODO: refine to dispatch-map timestamp.
    Int::from(0)
}

#[cfg(creusot)]
#[logic]
pub fn drive_of(_key: CacheKey, num_drives: usize) -> Int {
    pearlite! { (_key@ % num_drives@) }
}

#[cfg(creusot)]
#[logic]
pub fn capacity_ok(m: SpecModel, needed: usize) -> bool {
    pearlite! { m.memory_used@ + needed@ <= m.memory_capacity@ }
}

#[cfg(creusot)]
#[logic]
pub fn key_absent(m: SpecModel, key: CacheKey) -> bool {
    pearlite! { !contains_key(m, key) }
}

#[cfg(creusot)]
#[logic]
pub fn key_in_memory_tier(m: SpecModel, key: CacheKey) -> bool {
    pearlite! { contains_key(m, key) && state_of(m, key) == SpecEntryState::MemoryTier }
}

#[cfg(creusot)]
#[logic]
pub fn key_in_block_device(m: SpecModel, key: CacheKey) -> bool {
    pearlite! { contains_key(m, key) && state_of(m, key) == SpecEntryState::BlockDevice }
}

#[cfg(creusot)]
#[logic]
pub fn key_in_pending_write(m: SpecModel, key: CacheKey) -> bool {
    pearlite! { contains_key(m, key) && state_of(m, key) == SpecEntryState::PendingWrite }
}

#[cfg(creusot)]
#[logic]
pub fn is_not_initialized_error(err: DispatcherError) -> bool {
    pearlite! {
        match err {
            DispatcherError::NotInitialized(_) => true,
            _ => false,
        }
    }
}

#[cfg(creusot)]
#[logic]
pub fn is_key_not_found_error(err: DispatcherError) -> bool {
    pearlite! {
        match err {
            DispatcherError::KeyNotFound(_) => true,
            _ => false,
        }
    }
}

#[cfg(creusot)]
#[logic]
pub fn is_already_exists_error(err: DispatcherError) -> bool {
    pearlite! {
        match err {
            DispatcherError::AlreadyExists(_) => true,
            _ => false,
        }
    }
}

#[cfg(creusot)]
#[logic]
pub fn is_allocation_failed_error(err: DispatcherError) -> bool {
    pearlite! {
        match err {
            DispatcherError::AllocationFailed(_) => true,
            _ => false,
        }
    }
}

#[cfg(creusot)]
#[logic]
pub fn is_invalid_parameter_error(err: DispatcherError) -> bool {
    pearlite! {
        match err {
            DispatcherError::InvalidParameter(_) => true,
            _ => false,
        }
    }
}

// Error-preservation lemmas from the verification plan.

#[cfg(creusot)]
#[trusted]
#[requires(wf(pre))]
#[ensures(wf(post))]
#[ensures(is_already_exists_error(err) ==> post == pre)]
pub fn lemma_already_exists_preserves_state(
    pre: SpecModel,
    post: SpecModel,
    err: DispatcherError,
) {
}

#[cfg(creusot)]
#[trusted]
#[requires(wf(pre))]
#[ensures(wf(post))]
#[ensures(is_key_not_found_error(err) ==> post == pre)]
pub fn lemma_not_found_preserves_state(pre: SpecModel, post: SpecModel, err: DispatcherError) {}

#[cfg(creusot)]
#[trusted]
#[requires(wf(pre))]
#[ensures(wf(post))]
#[ensures(is_invalid_parameter_error(err) ==> post == pre)]
pub fn lemma_invalid_parameter_preserves_state(
    pre: SpecModel,
    post: SpecModel,
    err: DispatcherError,
) {
}

#[cfg(creusot)]
#[trusted]
#[requires(wf(pre))]
#[ensures(wf(post))]
#[ensures(is_allocation_failed_error(err) ==> !contains_key(post, key))]
pub fn lemma_allocation_failed_no_insertion(
    pre: SpecModel,
    post: SpecModel,
    key: CacheKey,
    err: DispatcherError,
) {
}

// Contract stubs for dispatcher methods.
// Each function models pre/post relations only; bodies are intentionally empty.

#[cfg(creusot)]
#[trusted]
#[requires(wf(pre))]
#[ensures(wf(post))]
#[ensures(result.is_ok() ==> post.initialized)]
pub fn contract_initialize(
    pre: SpecModel,
    post: SpecModel,
    _config: DispatcherConfig,
    result: Result<(), DispatcherError>,
) {
}

#[cfg(creusot)]
#[trusted]
#[requires(wf(pre))]
#[ensures(wf(post))]
#[ensures(result.is_ok() ==> !post.initialized)]
pub fn contract_shutdown(
    pre: SpecModel,
    post: SpecModel,
    result: Result<(), DispatcherError>,
) {
}

#[cfg(creusot)]
#[trusted]
#[requires(wf(pre))]
#[ensures(wf(post))]
#[ensures(result.is_ok() ==> result.unwrap() == contains_key(pre, key))]
#[ensures(post == pre)]
pub fn contract_check(
    pre: SpecModel,
    post: SpecModel,
    key: CacheKey,
    result: Result<bool, DispatcherError>,
) {
}

#[cfg(creusot)]
#[trusted]
#[requires(wf(pre))]
#[ensures(wf(post))]
#[ensures(result.is_ok() ==> key_in_memory_tier(post, key))]
#[ensures(matches!(result, Err(DispatcherError::AlreadyExists(_))) ==> post == pre)]
#[ensures(matches!(result, Err(DispatcherError::AllocationFailed(_))) ==> !contains_key(post, key))]
pub fn contract_populate(
    pre: SpecModel,
    post: SpecModel,
    key: CacheKey,
    _ipc_handle: IpcHandle,
    result: Result<(), DispatcherError>,
) {
}

#[cfg(creusot)]
#[trusted]
#[requires(wf(pre))]
#[ensures(wf(post))]
#[ensures(matches!(result, Err(DispatcherError::KeyNotFound(_))) ==> post == pre)]
#[ensures(result.is_ok() ==> contains_key(post, key))]
pub fn contract_lookup(
    pre: SpecModel,
    post: SpecModel,
    key: CacheKey,
    _ipc_handle: IpcHandle,
    result: Result<(), DispatcherError>,
) {
}

#[cfg(creusot)]
#[trusted]
#[requires(wf(pre))]
#[ensures(wf(post))]
#[ensures(result.is_ok() ==> key_absent(post, key))]
#[ensures(matches!(result, Err(DispatcherError::KeyNotFound(_))) ==> post == pre)]
pub fn contract_remove(
    pre: SpecModel,
    post: SpecModel,
    key: CacheKey,
    result: Result<(), DispatcherError>,
) {
}

#[cfg(creusot)]
#[trusted]
#[requires(wf(pre))]
#[ensures(wf(post))]
#[ensures(result.is_ok() ==> contains_key(post, key))]
#[ensures(result.is_ok() ==> timestamp_of(post, key) >= timestamp_of(pre, key))]
#[ensures(matches!(result, Err(DispatcherError::KeyNotFound(_))) ==> post == pre)]
pub fn contract_touch(
    pre: SpecModel,
    post: SpecModel,
    key: CacheKey,
    result: Result<(), DispatcherError>,
) {
}

#[cfg(creusot)]
#[trusted]
#[requires(wf(pre))]
#[ensures(wf(post))]
#[ensures(matches!(result, Err(DispatcherError::InvalidParameter(_))) ==> post == pre)]
#[ensures(matches!(result, Err(DispatcherError::AlreadyExists(_))) ==> post == pre)]
#[ensures(result.is_ok() ==> key_in_pending_write(post, key))]
pub fn contract_prepare_store(
    pre: SpecModel,
    post: SpecModel,
    key: CacheKey,
    size: u32,
    result: Result<(), DispatcherError>,
) {
}

#[cfg(creusot)]
#[trusted]
#[requires(wf(pre))]
#[ensures(wf(post))]
#[ensures(result.is_ok() ==> key_in_block_device(post, key))]
#[ensures(result.is_ok() ==> !key_in_pending_write(post, key))]
#[ensures(matches!(result, Err(DispatcherError::KeyNotFound(_))) ==> post == pre)]
pub fn contract_commit_store(
    pre: SpecModel,
    post: SpecModel,
    key: CacheKey,
    result: Result<(), DispatcherError>,
) {
}

#[cfg(creusot)]
#[trusted]
#[requires(wf(pre))]
#[ensures(wf(post))]
#[ensures(result.is_ok() ==> key_absent(post, key))]
#[ensures(result.is_ok() ==> !key_in_pending_write(post, key))]
#[ensures(matches!(result, Err(DispatcherError::KeyNotFound(_))) ==> post == pre)]
pub fn contract_cancel_store(
    pre: SpecModel,
    post: SpecModel,
    key: CacheKey,
    result: Result<(), DispatcherError>,
) {
}

#[cfg(creusot)]
#[trusted]
#[requires(wf(pre))]
#[requires(needed@ <= u32::MAX@)]
#[ensures(wf(post))]
#[ensures(attempts@ <= MAX_EVICT_ATTEMPTS@)]
#[ensures(result.is_ok() ==> capacity_ok(post, needed))]
pub fn contract_evict_for_space(
    pre: SpecModel,
    post: SpecModel,
    needed: usize,
    attempts: usize,
    result: Result<(), DispatcherError>,
) {
}

#[cfg(creusot)]
#[trusted]
#[requires(wf(pre))]
#[ensures(wf(post))]
#[ensures(result.is_ok() ==> (forall<k: Int> true))]
pub fn contract_clear_memory_tier(
    pre: SpecModel,
    post: SpecModel,
    result: Result<usize, DispatcherError>,
) {
}

#[cfg(creusot)]
#[trusted]
#[requires(wf(pre))]
#[ensures(wf(post))]
#[ensures(result.is_ok() ==> key_in_block_device(post, key))]
pub fn contract_recover_extent(
    pre: SpecModel,
    post: SpecModel,
    key: CacheKey,
    _offset: u64,
    _size_blocks: u32,
    result: Result<(), DispatcherError>,
) {
}

