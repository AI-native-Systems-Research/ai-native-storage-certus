//! Error-path and control-flow shape mirrors.
//!
//! These capture the *decision logic* of the error paths — which error variant
//! or branch is produced given a boundary outcome. The boundary events
//! themselves (block-device I/O, the `crc32fast` algorithm, receptacle binding,
//! namespace targeting) are the trusted TCB and are NOT proved here; the mirror
//! proves only the sequential mapping the source performs once the boundary has
//! reported.
//!
//! Covers `G4 DPM-IO-PROPAGATE`, `G8 DPM-CRC-INTEGRITY` (gate only),
//! `G9 DPM-NAMESPACE-ID` (selection only), `DPM-INIT-REQUIRES-BLOCKDEV`,
//! `DPM-FORMAT-REQUIRES-BLOCKDEV`, `DPM-INIT-BACKUP-FALLBACK`,
//! `DPM-INIT-BOTH-CORRUPT-NOPTBL`, the `initialize_or_format` branch logic, and
//! `DPM-IOF-CORRUPT-BRANCH-DEAD` (R1).

use creusot_std::prelude::{Clone, *};

/// Mirror of the `PartitionTableError` variants the error paths produce.
#[derive(Clone, Copy)]
pub enum PtErr {
    NotInitialized,
    NoPartitionTable,
    CorruptTable,
    IoError,
    LayoutError,
}

/// Outcome of a single boundary GPT read attempt (`try_read_gpt_at`), abstracted
/// to the three cases `read_gpt` dispatches on (gpt.rs:68-74).
#[derive(Clone, Copy)]
pub enum Attempt {
    Valid,   // Ok(table)
    Corrupt, // Err(CorruptTable) — CRC/parse mismatch
    Io,      // Err(IoError) or other non-Corrupt error
}

/// Which source the resolved table came from.
#[derive(Clone, Copy)]
pub enum Source {
    Primary,
    Backup,
}

/// **`DPM-INIT-REQUIRES-BLOCKDEV`** / **`DPM-FORMAT-REQUIRES-BLOCKDEV`**
/// (error-case). Mirror of `self.block_device.get().map_err(|_|
/// NotInitialized)?` (lib.rs:69-71 / :87-89): an unconnected receptacle yields
/// `NotInitialized` and no I/O follows; a connected one proceeds. The receptacle
/// binding itself is a framework boundary (trusted); this proves the mapping.
#[ensures(!connected ==> result == Err(PtErr::NotInitialized))]
#[ensures(connected ==> result == Ok(()))]
pub fn require_blockdev(connected: bool) -> Result<(), PtErr> {
    if !connected {
        return Err(PtErr::NotInitialized);
    }
    Ok(())
}

/// **G4 `DPM-IO-PROPAGATE`** (error-case). Mirror of the pervasive
/// `.map_err(|e| PartitionTableError::IoError(e.to_string()))` on every
/// block-device send/recv (e.g. gpt.rs:57, :448, :462, :469). Any boundary I/O
/// failure surfaces as `IoError`. The I/O effect itself is trusted; this proves
/// the error is never swallowed or mis-typed.
#[ensures(io_failed ==> result == Err(PtErr::IoError))]
#[ensures(!io_failed ==> result == Ok(()))]
pub fn map_io_err(io_failed: bool) -> Result<(), PtErr> {
    if io_failed {
        return Err(PtErr::IoError);
    }
    Ok(())
}

/// **G8 `DPM-CRC-INTEGRITY`** (gate only). Mirror of the CRC comparison gate
/// (gpt.rs:100-105 header, :114-119 entries): `if computed != stored { return
/// CorruptTable }`. The `crc32fast::hash` *algorithm* is a trusted boundary (its
/// correctness and the byte-domain layout are NOT proved); what is proved is
/// that a mismatch is rejected as `CorruptTable` and a match is accepted.
#[ensures(computed@ == stored@ ==> result == Ok(()))]
#[ensures(computed@ != stored@ ==> result == Err(PtErr::CorruptTable))]
pub fn crc_gate(computed: u32, stored: u32) -> Result<(), PtErr> {
    if computed != stored {
        return Err(PtErr::CorruptTable);
    }
    Ok(())
}

/// **G9 `DPM-NAMESPACE-ID`** (selection). Mirror of `get_ns_id`
/// (lib.rs:34-36): `self.ns_id.lock().unwrap().unwrap_or(1)`. Proves the
/// configured namespace id is used when set, else the default `1`. That the
/// selected id is what block I/O actually *targets* is an I/O-effect frame
/// (trusted); the selection arithmetic is proved.
#[ensures(configured == Some(n) ==> result == n)]
#[ensures(configured == None ==> result@ == 1)]
pub fn get_ns_id(configured: Option<u32>, n: u32) -> u32 {
    match configured {
        Some(id) => id,
        None => 1,
    }
}

/// **`DPM-INIT-BACKUP-FALLBACK`** + **`DPM-INIT-BOTH-CORRUPT-NOPTBL`** + **R1**.
/// Faithful mirror of `read_gpt` (gpt.rs:66-86):
/// ```ignore
/// match self.try_read_gpt_at(1, 2) {
///     Ok(table) => return Ok(table),
///     Err(CorruptTable(_)) => { /* fall through */ }
///     Err(e) => return Err(e),
/// }
/// self.try_read_gpt_at(backup..).map_err(|_| NoPartitionTable(..))
/// ```
/// Proves: a valid primary returns `Primary`; a non-Corrupt primary error
/// propagates verbatim (`Io`); a `Corrupt` primary falls through to the backup,
/// which returns `Backup` if valid and **remaps *every* backup failure to
/// `NoPartitionTable`** (R1) — so `CorruptTable` is never a terminal `read_gpt`
/// error.
#[ensures(primary == Attempt::Valid ==> result == Ok(Source::Primary))]
#[ensures(primary == Attempt::Io ==> result == Err(PtErr::IoError))]
#[ensures(primary == Attempt::Corrupt && backup == Attempt::Valid ==> result == Ok(Source::Backup))]
#[ensures(primary == Attempt::Corrupt && backup != Attempt::Valid ==> result == Err(PtErr::NoPartitionTable))]
// R1: read_gpt never terminates with CorruptTable.
#[ensures(result != Err(PtErr::CorruptTable))]
pub fn read_gpt(primary: Attempt, backup: Attempt) -> Result<Source, PtErr> {
    match primary {
        Attempt::Valid => return Ok(Source::Primary),
        Attempt::Corrupt => { /* fall through to backup */ }
        Attempt::Io => return Err(PtErr::IoError),
    }
    // Backup path: any failure is remapped to NoPartitionTable (gpt.rs:81-85).
    match backup {
        Attempt::Valid => Ok(Source::Backup),
        _ => Err(PtErr::NoPartitionTable),
    }
}

/// The `formatted` flag returned by `initialize_or_format`.
#[derive(Clone, Copy)]
pub enum IofOutcome {
    Existing,  // (table, formatted = false)
    Formatted, // (table, formatted = true)
    Failed,    // Err(e)
}

/// The outcome of the inner `self.initialize()` call, i.e. of `read_gpt`. Per R1
/// its terminal set is `{Valid, NoTable, Io}` — `Corrupt` is listed only because
/// the source code still branches on it (dead under R1).
#[derive(Clone, Copy)]
pub enum InitResult {
    Valid,
    NoTable,
    Corrupt,
    Io,
}

/// **`DPM-IOF-*`** (helper, out-of-N) + **`DPM-IOF-CORRUPT-BRANCH-DEAD`** (R1).
/// Faithful mirror of `initialize_or_format` (lib.rs:45-64). `init` is the
/// outcome of the inner `self.initialize()` (i.e. of `read_gpt` above, whose
/// terminal errors are only `Valid`/`Io`/`NoPartitionTable` — never `Corrupt`,
/// per R1). Proves:
/// - `force_format` ⇒ formats (`Formatted`);
/// - not forced, valid GPT ⇒ returns existing (`Existing`);
/// - not forced, `NoPartitionTable` ⇒ formats (`Formatted`);
/// - not forced, `Io` ⇒ propagates (`Failed`).
///
/// The precondition `init != Corrupt` encodes R1 (read_gpt never yields
/// CorruptTable); under it, the source's `CorruptTable` match arm (lib.rs:58) is
/// **unreachable** — the mirror still lists it, and the proof shows that arm is
/// never taken.
#[requires(init != InitResult::Corrupt)] // R1: initialize() never surfaces CorruptTable
#[ensures(force_format ==> result == IofOutcome::Formatted)]
#[ensures(!force_format && init == InitResult::Valid ==> result == IofOutcome::Existing)]
#[ensures(!force_format && init == InitResult::NoTable ==> result == IofOutcome::Formatted)]
#[ensures(!force_format && init == InitResult::Io ==> result == IofOutcome::Failed)]
pub fn initialize_or_format(force_format: bool, init: InitResult) -> IofOutcome {
    if force_format {
        return IofOutcome::Formatted;
    }
    match init {
        InitResult::Valid => IofOutcome::Existing,
        // NoPartitionTable | CorruptTable => format. The CorruptTable half is
        // dead code under R1 (init != Corrupt), but transcribed faithfully.
        InitResult::NoTable | InitResult::Corrupt => IofOutcome::Formatted,
        InitResult::Io => IofOutcome::Failed,
    }
}
