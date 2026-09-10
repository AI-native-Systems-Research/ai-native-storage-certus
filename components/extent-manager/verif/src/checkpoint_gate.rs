//! Checkpoint dirty-gating (FR-016/FR-030).
//!
//!  * **EM-CKPT-SKIP-CLEAN** (lib.rs:729-745): `run_checkpoint` first checks the
//!    dirty flag; when nothing has changed since the last successful checkpoint
//!    it returns `Ok(())` immediately, performing NO checkpoint-region or
//!    superblock I/O. Only a dirty state proceeds to serialize + write.
//!
//! Model: a `dirty: bool` guard. We prove the skip decision is exactly
//! `!dirty`, and that a completed checkpoint clears the flag (so a second,
//! back-to-back checkpoint is skipped). Re-authored from the inventory.

use creusot_std::prelude::*;

/// The observable outcome of entering `run_checkpoint`.
#[derive(DeepModel)]
pub enum CheckpointAction {
    /// Clean: returned Ok without doing any I/O (lib.rs:743).
    Skipped,
    /// Dirty: serialized regions + wrote checkpoint/superblock (lib.rs:747-321).
    Wrote,
}

/// **EM-CKPT-SKIP-CLEAN**: the branch taken is `Skipped` iff not dirty.
#[ensures(!dirty ==> result == CheckpointAction::Skipped)]
#[ensures(dirty ==> result == CheckpointAction::Wrote)]
pub fn checkpoint_action(dirty: bool) -> CheckpointAction {
    if !dirty {
        CheckpointAction::Skipped
    } else {
        CheckpointAction::Wrote
    }
}

/// A successful write clears the dirty flag (lib.rs:317-320): after a checkpoint
/// that wrote, the state is clean, so an immediate re-checkpoint is skipped.
#[ensures((^dirty) == false)]
pub fn clear_after_write(dirty: &mut bool) {
    *dirty = false;
}

/// Back-to-back property: run a checkpoint on a dirty state, then again. The
/// first writes and clears the flag; the second is skipped.
#[requires(*dirty)]
#[ensures(result == CheckpointAction::Skipped)]
pub fn double_checkpoint_second_skips(dirty: &mut bool) -> CheckpointAction {
    let first = checkpoint_action(*dirty);
    proof_assert!(first == CheckpointAction::Wrote);
    clear_after_write(dirty);
    checkpoint_action(*dirty)
}
