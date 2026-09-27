//! Creusot proof crate for the `logger` component (interface `ILogger`, 4 public methods).
//!
//! FAITHFUL PURE-CORE MIRROR of the decision logic in `components/logger/src/lib.rs`.
//! The product crate cannot be built under Creusot (chrono's `Utc::now`, the
//! `libc::isatty` FFI call, `Mutex<Box<dyn Write + Send>>`, `std::fs::OpenOptions`
//! real I/O, and the `define_component!` macro expansion are all outside Creusot's
//! model), so each Creusot-appropriate obligation is re-expressed as a
//! `#[requires]`/`#[ensures]` contract on a mirror of the corresponding source path.
//!
//! Every property gets a `verify_<id>` fn (id lower-cased, `-` -> `_`) plus a
//! `verify_<id>__mutant` anti-vacuity twin that MUST FAIL to prove.
//!
//! Fidelity classes (recorded per-id in the advisory side-file):
//!   * real-type        — the mirror is the source enum/boolean/arithmetic decision on
//!                        the same types, using the real operators.
//!   * ghost-mirror     — an opaque runtime object (`Mutex<Box<dyn Write + Send>>`) is
//!                        modelled by a scalar tag; the mirror proves the DECISION over
//!                        the tag, not the object.
//!   * trusted-boundary — the value is a syscall/FFI/I-O result (`libc::isatty`, the
//!                        filesystem open, `write_all`/`flush`); the value is trusted
//!                        and arrives as a parameter, and the in-crate MAPPING of that
//!                        value into the logger's behaviour is what is proved.
//!
//! NOT COVERED HERE, by construction: every obligation over concrete string CONTENT
//! (the `format!`-built line, the ISO-8601 timestamp text, the "ERROR"/"WARN "
//! tokens, the ANSI escape bytes, and the `from_env_str` match against string
//! literals). Creusot models `str`/`String` as an OPAQUE `Seq<char>`
//! (creusot-std/src/std/string.rs:5-41): the view exists but no axiom relates a
//! string to concrete characters, and `format!`/string literals have no model at
//! all. Those ids cite IX-CREUSOT-STRING-CONTENT in the advisory and are covered by
//! the Kani lane, which checks the concrete bytes.

use creusot_std::prelude::*;

// ===========================================================================
// LogLevel — real-type mirror of `logger::LogLevel` (src/lib.rs:35-41)
// ===========================================================================

#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}
use LogLevel::*;

/// Logical severity rank == the source enum's explicit discriminant
/// (`Error = 0, Warn = 1, Info = 2, Debug = 3`, lib.rs:37-40). The derived `Ord`
/// that the level filter compares (`level > self.state.level`, lib.rs:200) is
/// exactly the order of these discriminants.
#[logic(open)]
pub fn rk(l: LogLevel) -> Int {
    pearlite! {
        match l {
            Error => 0,
            Warn => 1,
            Info => 2,
            Debug => 3,
        }
    }
}

/// Executable counterpart of `rk`: the discriminant the comparison operates on.
#[ensures(result@ == rk(l))]
fn rank(l: LogLevel) -> u8 {
    match l {
        Error => 0,
        Warn => 1,
        Info => 2,
        Debug => 3,
    }
}

// LOG-LEVEL-ORDER — the levels are totally ordered Error < Warn < Info < Debug and
// THAT ordering is what the filter compares against (lib.rs:35-41, 200).
#[ensures(rk(Error) == 0 && rk(Warn) == 1 && rk(Info) == 2 && rk(Debug) == 3)]
#[ensures(rk(Error) < rk(Warn) && rk(Warn) < rk(Info) && rk(Info) < rk(Debug))]
// distinct levels have distinct ranks (the order is strict, not a pre-order)
#[ensures(forall<a: LogLevel, b: LogLevel> rk(a) == rk(b) ==> a == b)]
// and the value the filter actually compares IS that rank
#[ensures(result@ == rk(l))]
pub fn verify_log_level_order(l: LogLevel) -> u8 {
    rank(l)
}
// MUTANT: claims Warn is more severe than Error — false under the real discriminants.
#[ensures(rk(Warn) < rk(Error))]
#[ensures(result@ == rk(l))]
pub fn verify_log_level_order__mutant(l: LogLevel) -> u8 {
    rank(l)
}

// ===========================================================================
// The emit decision — mirror of `LoggerComponent::log` (src/lib.rs:199-219)
// ===========================================================================

/// Mirror of the level-filter gate at the head of `LoggerComponent::log`
/// (lib.rs:200-202): `if level > self.state.level { return; }`.
///
/// Returns `Some(level)` when a line IS emitted, carrying the level it is emitted
/// at, and `None` when the filter suppresses the call entirely. The line's TEXT is
/// deliberately not modelled (IX-CREUSOT-STRING-CONTENT); what is modelled is
/// whether a line is produced and at which level.
#[ensures(rk(level) > rk(threshold) ==> result == None)]
#[ensures(rk(level) <= rk(threshold) ==> result == Some(level))]
fn log_core(level: LogLevel, threshold: LogLevel) -> Option<LogLevel> {
    if rank(level) > rank(threshold) {
        return None;
    }
    Some(level)
}

// ---- LOG-ERROR-LEVEL-TAG — `error(msg)` forwards to log with LogLevel::Error ----
// (lib.rs:223-225). Error is rank 0, so it is never suppressed: the call ALWAYS
// emits, and always at the Error level.
#[ensures(forall<l: LogLevel> result == Some(l) ==> l == Error)]
#[ensures(rk(Error) <= rk(threshold) ==> result == Some(Error))]
pub fn verify_log_error_level_tag(threshold: LogLevel) -> Option<LogLevel> {
    log_core(Error, threshold) // ILogger::error
}
// MUTANT: claims the line comes out at Warn.
#[ensures(forall<l: LogLevel> result == Some(l) ==> l == Warn)]
#[ensures(rk(Error) <= rk(threshold) ==> result == Some(Warn))]
pub fn verify_log_error_level_tag__mutant(threshold: LogLevel) -> Option<LogLevel> {
    log_core(Error, threshold)
}

// ---- LOG-WARN-LEVEL-TAG — `warn(msg)` forwards with LogLevel::Warn (lib.rs:227-229) ----
#[ensures(forall<l: LogLevel> result == Some(l) ==> l == Warn)]
#[ensures(rk(Warn) <= rk(threshold) ==> result == Some(Warn))]
pub fn verify_log_warn_level_tag(threshold: LogLevel) -> Option<LogLevel> {
    log_core(Warn, threshold) // ILogger::warn
}
#[ensures(forall<l: LogLevel> result == Some(l) ==> l == Error)]
#[ensures(rk(Warn) <= rk(threshold) ==> result == Some(Error))]
pub fn verify_log_warn_level_tag__mutant(threshold: LogLevel) -> Option<LogLevel> {
    log_core(Warn, threshold)
}

// ---- LOG-INFO-LEVEL-TAG — `info(msg)` forwards with LogLevel::Info (lib.rs:231-233) ----
#[ensures(forall<l: LogLevel> result == Some(l) ==> l == Info)]
#[ensures(rk(Info) <= rk(threshold) ==> result == Some(Info))]
pub fn verify_log_info_level_tag(threshold: LogLevel) -> Option<LogLevel> {
    log_core(Info, threshold) // ILogger::info
}
#[ensures(forall<l: LogLevel> result == Some(l) ==> l == Debug)]
#[ensures(rk(Info) <= rk(threshold) ==> result == Some(Debug))]
pub fn verify_log_info_level_tag__mutant(threshold: LogLevel) -> Option<LogLevel> {
    log_core(Info, threshold)
}

// ---- LOG-DEBUG-LEVEL-TAG — `debug(msg)` forwards with LogLevel::Debug (lib.rs:235-237) ----
#[ensures(forall<l: LogLevel> result == Some(l) ==> l == Debug)]
#[ensures(rk(Debug) <= rk(threshold) ==> result == Some(Debug))]
pub fn verify_log_debug_level_tag(threshold: LogLevel) -> Option<LogLevel> {
    log_core(Debug, threshold) // ILogger::debug
}
#[ensures(forall<l: LogLevel> result == Some(l) ==> l == Info)]
#[ensures(rk(Debug) <= rk(threshold) ==> result == Some(Info))]
pub fn verify_log_debug_level_tag__mutant(threshold: LogLevel) -> Option<LogLevel> {
    log_core(Debug, threshold)
}

// ---- LOG-FILTER-SUPPRESS — more verbose than the threshold => NO output at all ----
#[requires(rk(level) > rk(threshold))]
#[ensures(result == None)]
pub fn verify_log_filter_suppress(level: LogLevel, threshold: LogLevel) -> Option<LogLevel> {
    log_core(level, threshold)
}
// MUTANT: claims a suppressed call still emits.
#[requires(rk(level) > rk(threshold))]
#[ensures(exists<l: LogLevel> result == Some(l))]
pub fn verify_log_filter_suppress__mutant(
    level: LogLevel,
    threshold: LogLevel,
) -> Option<LogLevel> {
    log_core(level, threshold)
}

// ---- LOG-FILTER-EMIT — at or above the threshold (equal or more severe) => emitted ----
#[requires(rk(level) <= rk(threshold))]
#[ensures(result == Some(level))]
pub fn verify_log_filter_emit(level: LogLevel, threshold: LogLevel) -> Option<LogLevel> {
    log_core(level, threshold)
}
// MUTANT: claims an at-threshold call is suppressed (would make the filter exclusive).
#[requires(rk(level) <= rk(threshold))]
#[ensures(result == None)]
pub fn verify_log_filter_emit__mutant(level: LogLevel, threshold: LogLevel) -> Option<LogLevel> {
    log_core(level, threshold)
}

// ===========================================================================
// Threshold selection — mirror of `LogLevel::from_env` (src/lib.rs:68-73)
// ===========================================================================

/// Mirror of `LogLevel::from_env` (lib.rs:68-73). `var` is the ALREADY-PARSED
/// `RUST_LOG` value: `None` models `Err(_)` (the variable is not set), `Some(l)`
/// models `Ok(val)` whose text parsed to `l`. The string PARSING itself
/// (`from_env_str`, lib.rs:57-66) is concrete string content and therefore
/// IX-CREUSOT-STRING-CONTENT — it is the Kani lane's obligation, not modelled here.
#[ensures(var == None ==> result == Info)]
#[ensures(forall<l: LogLevel> var == Some(l) ==> result == l)]
fn from_env_m(var: Option<LogLevel>) -> LogLevel {
    match var {
        Some(l) => l,
        None => Info,
    }
}

// ---- LOG-DEFAULT-LEVEL-INFO — RUST_LOG unset => the threshold is Info ----
#[requires(var == None)]
#[ensures(result == Info)]
pub fn verify_log_default_level_info(var: Option<LogLevel>) -> LogLevel {
    from_env_m(var)
}
// MUTANT: claims the unset default is Debug (i.e. everything logs by default).
#[requires(var == None)]
#[ensures(result == Debug)]
pub fn verify_log_default_level_info__mutant(var: Option<LogLevel>) -> LogLevel {
    from_env_m(var)
}

// ===========================================================================
// Writer destination + colour — mirror of `LoggerState` (src/lib.rs:104-121)
//                               and `new_with_file` (src/lib.rs:155-165)
// ===========================================================================

/// GHOST TAG for the opaque `writer: Mutex<Box<dyn Write + Send>>` (lib.rs:105).
/// A trait object behind a `Mutex` cannot be processed by Creusot, so the writer is
/// modelled by the DESTINATION the constructor selected. The proofs below establish
/// which destination each constructor picks — not the writer object itself.
pub enum WriterKind {
    Stderr,
    File,
}
use WriterKind::*;

/// Mirror of the three `LoggerState` fields (lib.rs:104-108).
pub struct StateM {
    pub writer: WriterKind,
    pub level: LogLevel,
    pub use_color: bool,
}

/// Mirror of `impl Default for LoggerState` (lib.rs:110-121).
///
/// `is_tty` is the value returned by `unsafe { libc::isatty(libc::STDERR_FILENO) } != 0`
/// (lib.rs:114). That FFI call is outside Creusot (NV-6) so its result is TRUSTED and
/// arrives as a parameter; what is proved is the MAPPING of that result into
/// `use_color`, together with the writer and threshold the constructor installs.
#[ensures(result.writer == Stderr)]
#[ensures(result.use_color == is_tty)]
#[ensures(var == None ==> result.level == Info)]
#[ensures(forall<l: LogLevel> var == Some(l) ==> result.level == l)]
fn state_default_m(is_tty: bool, var: Option<LogLevel>) -> StateM {
    StateM {
        writer: Stderr,          // lib.rs:116  Mutex::new(Box::new(io::stderr()))
        level: from_env_m(var),  // lib.rs:117  LogLevel::from_env()
        use_color: is_tty,       // lib.rs:118  use_color: is_tty
    }
}

// ---- LOG-CONSOLE-DEFAULT-STDERR — with no file configured, output goes to stderr ----
#[ensures(result == Stderr)]
pub fn verify_log_console_default_stderr(is_tty: bool, var: Option<LogLevel>) -> WriterKind {
    state_default_m(is_tty, var).writer
}
// MUTANT: claims the default destination is the file writer.
#[ensures(result == File)]
pub fn verify_log_console_default_stderr__mutant(
    is_tty: bool,
    var: Option<LogLevel>,
) -> WriterKind {
    state_default_m(is_tty, var).writer
}

// ---- LOG-CONSOLE-COLOR-AUTODETECT — colour on exactly when stderr is a terminal ----
#[ensures(result == is_tty)]
pub fn verify_log_console_color_autodetect(is_tty: bool, var: Option<LogLevel>) -> bool {
    state_default_m(is_tty, var).use_color
}
// MUTANT: claims colour is always on, i.e. also when stderr is NOT a terminal.
#[ensures(result == true)]
pub fn verify_log_console_color_autodetect__mutant(is_tty: bool, var: Option<LogLevel>) -> bool {
    state_default_m(is_tty, var).use_color
}

// ---------------------------------------------------------------------------
// `OpenOptions` builder mirror — `new_with_file` (src/lib.rs:156-159)
// ---------------------------------------------------------------------------

/// Mirror of the four `std::fs::OpenOptions` flags that matter here, with the REAL
/// builder semantics: every flag starts `false`, and each setter changes ONLY its
/// own flag. The filesystem effect is I/O outside Creusot; this models the
/// CONFIGURATION the code builds before handing it to the OS.
pub struct OpenOptionsM {
    pub create: bool,
    pub append: bool,
    pub truncate: bool,
    pub write: bool,
}

#[ensures(result.create == false && result.append == false)]
#[ensures(result.truncate == false && result.write == false)]
fn oo_new() -> OpenOptionsM {
    OpenOptionsM { create: false, append: false, truncate: false, write: false }
}

#[ensures(result.create == v)]
#[ensures(result.append == o.append && result.truncate == o.truncate && result.write == o.write)]
fn oo_create(o: OpenOptionsM, v: bool) -> OpenOptionsM {
    OpenOptionsM { create: v, append: o.append, truncate: o.truncate, write: o.write }
}

#[ensures(result.append == v)]
#[ensures(result.create == o.create && result.truncate == o.truncate && result.write == o.write)]
fn oo_append(o: OpenOptionsM, v: bool) -> OpenOptionsM {
    OpenOptionsM { create: o.create, append: v, truncate: o.truncate, write: o.write }
}

/// Mirror of the exact chain at lib.rs:156-159:
/// `OpenOptions::new().create(true).append(true)`.
#[ensures(result.create == true && result.append == true)]
#[ensures(result.truncate == false)]
fn open_options_m() -> OpenOptionsM {
    oo_append(oo_create(oo_new(), true), true)
}

// ---- LOG-FILE-CREATE — a missing file is created, and the file is opened for append ----
// Returns (create, append, truncate) as configured. The filesystem CREATE/APPEND
// effect is real I/O (trusted-boundary); what is proved is the configuration
// decision the code makes — including that `truncate` is never set, so an existing
// log is appended to rather than discarded.
#[ensures(result.0 == true)]
#[ensures(result.1 == true)]
#[ensures(result.2 == false)]
pub fn verify_log_file_create() -> (bool, bool, bool) {
    let o = open_options_m();
    (o.create, o.append, o.truncate)
}
// MUTANT: claims the file is NOT created on demand.
#[ensures(result.0 == false)]
pub fn verify_log_file_create__mutant() -> (bool, bool, bool) {
    let o = open_options_m();
    (o.create, o.append, o.truncate)
}

// ---------------------------------------------------------------------------
// `LoggerComponent::new_with_file` — src/lib.rs:155-165
// ---------------------------------------------------------------------------

/// Mirror of `LoggerComponent::new_with_file` (lib.rs:155-165).
///
/// The filesystem open is real I/O outside Creusot, so its outcome arrives as the
/// `open` parameter (TRUSTED): `Ok(fd)` is a successfully opened file and `Err(e)` an
/// `io::Error` modelled as an opaque `u8` payload. What is PROVED is the in-crate
/// control flow around it: the `?` at lib.rs:159 returns that error UNCHANGED and
/// builds no logger, and on success the installed state is
/// (File destination, `from_env()` threshold, `use_color: false`).
#[ensures(forall<e: u8> open == Err(e) ==> result == Err(e))]
#[ensures(forall<s: StateM> result == Ok(s) ==> s.writer == File)]
#[ensures(forall<s: StateM> result == Ok(s) ==> s.use_color == false)]
#[ensures(forall<f: u8> open == Ok(f) ==> exists<s: StateM> result == Ok(s))]
fn new_with_file_m(open: Result<u8, u8>, var: Option<LogLevel>) -> Result<StateM, u8> {
    let _file = match open {
        Ok(f) => f,
        Err(e) => return Err(e), // lib.rs:159  `.open(path)?`
    };
    Ok(StateM {
        writer: File,            // lib.rs:161  Mutex::new(Box::new(file))
        level: from_env_m(var),  // lib.rs:162  LogLevel::from_env()
        use_color: false,        // lib.rs:163  use_color: false
    })
}

// ---- LOG-FILE-OUTPUT-MODE — the logger can be configured to write to a named file ----
#[requires(exists<f: u8> open == Ok(f))]
#[ensures(exists<s: StateM> result == Ok(s))]
#[ensures(forall<s: StateM> result == Ok(s) ==> s.writer == File)]
pub fn verify_log_file_output_mode(open: Result<u8, u8>, var: Option<LogLevel>) -> Result<StateM, u8> {
    new_with_file_m(open, var)
}
// MUTANT: claims the file constructor still selects stderr.
#[requires(exists<f: u8> open == Ok(f))]
#[ensures(forall<s: StateM> result == Ok(s) ==> s.writer == Stderr)]
pub fn verify_log_file_output_mode__mutant(
    open: Result<u8, u8>,
    var: Option<LogLevel>,
) -> Result<StateM, u8> {
    new_with_file_m(open, var)
}

// ---- LOG-FILE-COLOR-DISABLED — file output has colour off, whatever the environment ----
// Note that `is_tty` is not even an input to this path: `new_with_file` hard-wires
// `use_color: false` (lib.rs:163), so no environment can turn colour on for a file.
// The `requires` keeps the obligation about a logger that WAS constructed, so it can
// never be discharged by the no-logger (failed-open) case.
#[requires(exists<f: u8> open == Ok(f))]
#[ensures(exists<s: StateM> result == Ok(s))]
#[ensures(forall<s: StateM> result == Ok(s) ==> s.use_color == false)]
pub fn verify_log_file_color_disabled(
    open: Result<u8, u8>,
    var: Option<LogLevel>,
) -> Result<StateM, u8> {
    new_with_file_m(open, var)
}
// MUTANT: claims a file logger may have colour enabled.
#[requires(exists<f: u8> open == Ok(f))]
#[ensures(forall<s: StateM> result == Ok(s) ==> s.use_color == true)]
pub fn verify_log_file_color_disabled__mutant(
    open: Result<u8, u8>,
    var: Option<LogLevel>,
) -> Result<StateM, u8> {
    new_with_file_m(open, var)
}

// ---- LOG-FILE-OPEN-ERROR — an unopenable path yields the underlying error, no logger ----
#[requires(exists<e: u8> open == Err(e))]
#[ensures(forall<e: u8> open == Err(e) ==> result == Err(e))]
#[ensures(exists<e: u8> result == Err(e))]
pub fn verify_log_file_open_error(open: Result<u8, u8>, var: Option<LogLevel>) -> Result<StateM, u8> {
    new_with_file_m(open, var)
}
// MUTANT: claims a failed open still hands back a logger.
#[requires(exists<e: u8> open == Err(e))]
#[ensures(exists<s: StateM> result == Ok(s))]
pub fn verify_log_file_open_error__mutant(
    open: Result<u8, u8>,
    var: Option<LogLevel>,
) -> Result<StateM, u8> {
    new_with_file_m(open, var)
}

// ===========================================================================
// The emit tail — mirror of src/lib.rs:216-218
//   let mut writer = self.state.writer.lock().unwrap();
//   let _ = writer.write_all(line.as_bytes());
//   let _ = writer.flush();
// ===========================================================================

// ---- LOG-WRITE-ERROR-IGNORED — a failed write/flush never panics or propagates ----
/// The real `write_all`/`flush` are I/O (trusted-boundary), so their `Result`s arrive
/// as parameters and range over every outcome. What is PROVED is that the tail is
/// TOTAL: both results are discarded with `let _ =`, so `log()` returns normally on
/// every combination of I/O outcomes — nothing is unwrapped and nothing is propagated
/// (note that `log` returns `()`, so it has no channel to propagate on).
#[ensures(result == true)]
fn emit_tail_m(w: Result<u8, u8>, f: Result<u8, u8>) -> bool {
    let _ = w; // lib.rs:217  let _ = writer.write_all(line.as_bytes());
    let _ = f; // lib.rs:218  let _ = writer.flush();
    true // control reaches the end of log() on every path
}

#[ensures(result == true)]
pub fn verify_log_write_error_ignored(w: Result<u8, u8>, f: Result<u8, u8>) -> bool {
    emit_tail_m(w, f)
}
// MUTANT: unwraps the write result instead of discarding it. `Result::unwrap` carries
// `#[requires(exists<t> self == Ok(t))]` in creusot-std, so the no-panic VC fails for
// an `Err` write — which is exactly the behaviour this property forbids.
#[ensures(result == true)]
pub fn verify_log_write_error_ignored__mutant(w: Result<u8, u8>, f: Result<u8, u8>) -> bool {
    let _v = w.unwrap();
    let _ = f;
    true
}

// ---- LOG-FLUSH-EACH-LINE — the flush unconditionally follows the write ----
/// Phase machine over the emit tail. The real `flush` is I/O (trusted-boundary); what
/// is PROVED is the SEQUENCING: `Start -> Wrote -> Flushed` with no branch between the
/// two steps, so a line that has been written is never left buffered — not even when
/// the write itself reported an error.
pub enum Phase {
    Start,
    Wrote,
    Flushed,
}
use Phase::*;

#[requires(p == Start)]
#[ensures(result == Flushed)]
fn emit_seq_m(p: Phase, w: Result<u8, u8>) -> Phase {
    let p = match p {
        Start => Wrote, // lib.rs:217  write_all
        other => other,
    };
    let _ = w; // its Result is discarded, so it cannot steer control flow
    match p {
        Wrote => Flushed, // lib.rs:218  flush — reached unconditionally
        other => other,
    }
}

#[requires(p == Start)]
#[ensures(result == Flushed)]
pub fn verify_log_flush_each_line(p: Phase, w: Result<u8, u8>) -> Phase {
    emit_seq_m(p, w)
}
// MUTANT: makes the flush conditional on a successful write, so a failed write would
// leave the line buffered. The Err path then stops at Wrote and the proof fails.
#[requires(p == Start)]
#[ensures(result == Flushed)]
pub fn verify_log_flush_each_line__mutant(p: Phase, w: Result<u8, u8>) -> Phase {
    let p = match p {
        Start => Wrote,
        other => other,
    };
    match w {
        Ok(_) => match p {
            Wrote => Flushed,
            other => other,
        },
        Err(_) => p, // flush skipped
    }
}
