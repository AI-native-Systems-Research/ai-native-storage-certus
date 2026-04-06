//! Generic logging handler for the actor framework.
//!
//! Provides [`LogLevel`], [`LogMessage`], and [`LogHandler`] — a reusable
//! [`ActorHandler`] that writes timestamped log lines to stderr and optionally
//! to a file.
//!
//! # Examples
//!
//! ```
//! use component_core::log::{LogHandler, LogMessage};
//! use component_core::actor::Actor;
//!
//! let actor = Actor::new(LogHandler::new(), |_| {});
//! let handle = actor.activate().unwrap();
//! handle.send(LogMessage::info("hello world")).unwrap();
//! handle.deactivate().unwrap();
//! ```

use crate::actor::ActorHandler;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Severity level for a log message.
///
/// Levels are ordered: `Debug < Info < Warn < Error`.
///
/// # Examples
///
/// ```
/// use component_core::log::LogLevel;
///
/// assert!(LogLevel::Debug < LogLevel::Info);
/// assert!(LogLevel::Warn < LogLevel::Error);
/// assert_eq!(format!("{}", LogLevel::Info), "INFO ");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    /// Verbose diagnostic information.
    Debug,
    /// General informational messages.
    Info,
    /// Potential issues that deserve attention.
    Warn,
    /// Errors that need immediate attention.
    Error,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LogLevel::Debug => write!(f, "DEBUG"),
            LogLevel::Info => write!(f, "INFO "),
            LogLevel::Warn => write!(f, "WARN "),
            LogLevel::Error => write!(f, "ERROR"),
        }
    }
}

/// A single log message carrying a severity level and text.
///
/// Construct using the convenience methods [`LogMessage::debug`],
/// [`LogMessage::info`], [`LogMessage::warn`], or [`LogMessage::error`].
///
/// # Examples
///
/// ```
/// use component_core::log::{LogLevel, LogMessage};
///
/// let msg = LogMessage::info("server started");
/// assert_eq!(msg.level(), LogLevel::Info);
/// assert_eq!(msg.text(), "server started");
/// ```
#[derive(Debug, Clone)]
pub struct LogMessage {
    level: LogLevel,
    text: String,
}

impl LogMessage {
    /// Create a debug-level message.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::log::{LogLevel, LogMessage};
    /// let msg = LogMessage::debug("trace data");
    /// assert_eq!(msg.level(), LogLevel::Debug);
    /// ```
    pub fn debug(text: impl Into<String>) -> Self {
        Self {
            level: LogLevel::Debug,
            text: text.into(),
        }
    }

    /// Create an info-level message.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::log::{LogLevel, LogMessage};
    /// let msg = LogMessage::info("ready");
    /// assert_eq!(msg.level(), LogLevel::Info);
    /// ```
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            level: LogLevel::Info,
            text: text.into(),
        }
    }

    /// Create a warn-level message.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::log::{LogLevel, LogMessage};
    /// let msg = LogMessage::warn("high latency");
    /// assert_eq!(msg.level(), LogLevel::Warn);
    /// ```
    pub fn warn(text: impl Into<String>) -> Self {
        Self {
            level: LogLevel::Warn,
            text: text.into(),
        }
    }

    /// Create an error-level message.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::log::{LogLevel, LogMessage};
    /// let msg = LogMessage::error("connection lost");
    /// assert_eq!(msg.level(), LogLevel::Error);
    /// ```
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            level: LogLevel::Error,
            text: text.into(),
        }
    }

    /// Returns the severity level of this message.
    pub fn level(&self) -> LogLevel {
        self.level
    }

    /// Returns the text content of this message.
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Actor handler that writes timestamped log messages to stderr and optionally
/// to a file.
///
/// Each log line has the format:
///
/// ```text
/// 2026-04-01T14:23:05.123Z [INFO ] message text
/// ```
///
/// Construct with [`LogHandler::new`] for stderr-only output, or
/// [`LogHandler::with_file`] to add file output. Use [`LogHandler::with_min_level`]
/// to filter messages below a threshold.
///
/// # Examples
///
/// ```
/// use component_core::log::{LogHandler, LogLevel, LogMessage};
/// use component_core::actor::Actor;
///
/// // stderr-only handler that filters below Warn
/// let handler = LogHandler::new().with_min_level(LogLevel::Warn);
/// let actor = Actor::new(handler, |_| {});
/// let handle = actor.activate().unwrap();
/// handle.send(LogMessage::info("filtered out")).unwrap();
/// handle.send(LogMessage::error("this appears")).unwrap();
/// handle.deactivate().unwrap();
/// ```
pub struct LogHandler {
    file: Option<BufWriter<File>>,
    min_level: LogLevel,
}

impl fmt::Debug for LogHandler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LogHandler")
            .field("file", &self.file.as_ref().map(|_| "<file>"))
            .field("min_level", &self.min_level)
            .finish()
    }
}

impl LogHandler {
    /// Create a handler that writes to stderr only.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::log::LogHandler;
    /// let handler = LogHandler::new();
    /// ```
    pub fn new() -> Self {
        Self {
            file: None,
            min_level: LogLevel::Debug,
        }
    }

    /// Create a handler that writes to both stderr and the given file path.
    ///
    /// The file is opened in append mode. If it does not exist, it is created.
    ///
    /// # Errors
    ///
    /// Returns [`io::Error`] if the file cannot be opened.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use component_core::log::LogHandler;
    /// let handler = LogHandler::with_file("/tmp/app.log").unwrap();
    /// ```
    pub fn with_file(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            file: Some(BufWriter::new(file)),
            min_level: LogLevel::Debug,
        })
    }

    /// Set the minimum severity level. Messages below this level are dropped.
    ///
    /// The default is [`LogLevel::Debug`] (all messages pass through).
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::log::{LogHandler, LogLevel};
    /// let handler = LogHandler::new().with_min_level(LogLevel::Warn);
    /// ```
    pub fn with_min_level(mut self, level: LogLevel) -> Self {
        self.min_level = level;
        self
    }
}

impl Default for LogHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Format a `SystemTime` as an RFC-3339 timestamp: `YYYY-MM-DDTHH:MM:SS.mmmZ`.
fn format_timestamp(time: SystemTime) -> String {
    let dur = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let total_secs = dur.as_secs();
    let millis = dur.subsec_millis();

    // Days since epoch
    let days = total_secs / 86400;
    let day_secs = total_secs % 86400;
    let hours = day_secs / 3600;
    let minutes = (day_secs % 3600) / 60;
    let seconds = day_secs % 60;

    // Convert days to Y-M-D using a civil calendar algorithm.
    // Based on Howard Hinnant's algorithm (public domain).
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        y, m, d, hours, minutes, seconds, millis
    )
}

impl ActorHandler<LogMessage> for LogHandler {
    fn handle(&mut self, msg: LogMessage) {
        if msg.level < self.min_level {
            return;
        }

        let timestamp = format_timestamp(SystemTime::now());
        let line = format!("{} [{}] {}", timestamp, msg.level, msg.text);

        // Write to stderr
        let _ = writeln!(io::stderr(), "{line}");

        // Write to file if configured
        if let Some(ref mut writer) = self.file {
            let _ = writeln!(writer, "{line}");
        }
    }

    fn on_stop(&mut self) {
        // Flush the file buffer to ensure all lines are written.
        if let Some(ref mut writer) = self.file {
            let _ = writer.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::Actor;
    use std::fs;

    #[test]
    fn log_level_ordering() {
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Error);
    }

    #[test]
    fn log_level_display() {
        assert_eq!(format!("{}", LogLevel::Debug), "DEBUG");
        assert_eq!(format!("{}", LogLevel::Info), "INFO ");
        assert_eq!(format!("{}", LogLevel::Warn), "WARN ");
        assert_eq!(format!("{}", LogLevel::Error), "ERROR");
    }

    #[test]
    fn log_message_constructors() {
        let d = LogMessage::debug("d");
        assert_eq!(d.level(), LogLevel::Debug);
        assert_eq!(d.text(), "d");

        let i = LogMessage::info("i");
        assert_eq!(i.level(), LogLevel::Info);

        let w = LogMessage::warn("w");
        assert_eq!(w.level(), LogLevel::Warn);

        let e = LogMessage::error("e");
        assert_eq!(e.level(), LogLevel::Error);
    }

    #[test]
    fn handler_writes_to_file() {
        let path = "/tmp/component_log_test_write.log";
        let _ = fs::remove_file(path);

        let handler = LogHandler::with_file(path).unwrap();
        let actor = Actor::new(handler, |_| {});
        let handle = actor.activate().unwrap();

        handle.send(LogMessage::info("hello")).unwrap();
        handle.send(LogMessage::warn("caution")).unwrap();
        handle.deactivate().unwrap();

        let contents = fs::read_to_string(path).unwrap();
        assert!(contents.contains("[INFO ] hello"));
        assert!(contents.contains("[WARN ] caution"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn handler_min_level_filters() {
        let path = "/tmp/component_log_test_filter.log";
        let _ = fs::remove_file(path);

        let handler = LogHandler::with_file(path)
            .unwrap()
            .with_min_level(LogLevel::Warn);
        let actor = Actor::new(handler, |_| {});
        let handle = actor.activate().unwrap();

        handle.send(LogMessage::debug("should not appear")).unwrap();
        handle.send(LogMessage::info("should not appear")).unwrap();
        handle.send(LogMessage::warn("visible")).unwrap();
        handle.send(LogMessage::error("also visible")).unwrap();
        handle.deactivate().unwrap();

        let contents = fs::read_to_string(path).unwrap();
        assert!(!contents.contains("should not appear"));
        assert!(contents.contains("[WARN ] visible"));
        assert!(contents.contains("[ERROR] also visible"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn handler_on_stop_flushes() {
        let path = "/tmp/component_log_test_flush.log";
        let _ = fs::remove_file(path);

        let handler = LogHandler::with_file(path).unwrap();
        let actor = Actor::new(handler, |_| {});
        let handle = actor.activate().unwrap();
        handle.send(LogMessage::info("flushed")).unwrap();
        handle.deactivate().unwrap();

        // After deactivate (which calls on_stop), file must be flushed.
        let contents = fs::read_to_string(path).unwrap();
        assert!(contents.contains("flushed"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn handler_default() {
        let h = LogHandler::default();
        assert!(h.file.is_none());
        assert_eq!(h.min_level, LogLevel::Debug);
    }

    #[test]
    fn format_timestamp_produces_valid_output() {
        let ts = format_timestamp(SystemTime::now());
        // Basic format check: YYYY-MM-DDTHH:MM:SS.mmmZ
        assert_eq!(ts.len(), 24);
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[7..8], "-");
        assert_eq!(&ts[10..11], "T");
        assert_eq!(&ts[13..14], ":");
        assert_eq!(&ts[16..17], ":");
        assert_eq!(&ts[19..20], ".");
        assert_eq!(&ts[23..24], "Z");
    }
}
