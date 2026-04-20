//! Logger component for the Certus storage system.
//!
//! Provides console and file-based logging with configurable log levels
//! via the `RUST_LOG` environment variable. Built with the component
//! framework using `define_component!`.
//!
//! # Quick start
//!
//! ```
//! use logger::LoggerComponentV1;
//! use interfaces::ILogger;
//! use component_core::query_interface;
//!
//! let component = LoggerComponentV1::new_default();
//! let log = query_interface!(component, ILogger).unwrap();
//! log.info("system ready");
//! ```

use chrono::{SecondsFormat, Utc};
use component_framework::define_component;
use interfaces::ILogger;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

/// Log severity levels, ordered from most to least severe.
///
/// # Examples
///
/// ```
/// use logger::LogLevel;
///
/// let level = LogLevel::from_env_str("info");
/// assert!(matches!(level, LogLevel::Info));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}

impl LogLevel {
    /// Parse a log level from a string (case-insensitive).
    ///
    /// Returns `Info` for unrecognized values. Maps "trace" to `Debug`.
    ///
    /// # Examples
    ///
    /// ```
    /// use logger::LogLevel;
    ///
    /// assert!(matches!(LogLevel::from_env_str("ERROR"), LogLevel::Error));
    /// assert!(matches!(LogLevel::from_env_str("trace"), LogLevel::Debug));
    /// assert!(matches!(LogLevel::from_env_str("invalid"), LogLevel::Info));
    /// ```
    pub fn from_env_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "error" => LogLevel::Error,
            "warn" | "warning" => LogLevel::Warn,
            "info" => LogLevel::Info,
            "debug" => LogLevel::Debug,
            "trace" => LogLevel::Debug,
            _ => LogLevel::Info,
        }
    }

    fn from_env() -> Self {
        match std::env::var("RUST_LOG") {
            Ok(val) => Self::from_env_str(&val),
            Err(_) => LogLevel::Info,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            LogLevel::Error => "ERROR",
            LogLevel::Warn => "WARN ",
            LogLevel::Info => "INFO ",
            LogLevel::Debug => "DEBUG",
        }
    }

    fn ansi_color(self) -> &'static str {
        match self {
            LogLevel::Error => "\x1b[31m",
            LogLevel::Warn => "\x1b[33m",
            LogLevel::Info => "\x1b[32m",
            LogLevel::Debug => "\x1b[36m",
        }
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

const ANSI_RESET: &str = "\x1b[0m";

/// Internal logger state wrapping the output writer, level threshold,
/// and color mode.
pub struct LoggerState {
    writer: Mutex<Box<dyn Write + Send>>,
    level: LogLevel,
    use_color: bool,
}

impl Default for LoggerState {
    fn default() -> Self {
        // SAFETY: isatty is a POSIX function that checks if fd 2 (stderr)
        // is a terminal. Always safe to call.
        let is_tty = unsafe { libc::isatty(libc::STDERR_FILENO) } != 0;
        Self {
            writer: Mutex::new(Box::new(io::stderr())),
            level: LogLevel::from_env(),
            use_color: is_tty,
        }
    }
}

define_component! {
    pub LoggerComponentV1 {
        version: "0.1.0",
        provides: [ILogger],
        fields: {
            state: LoggerState,
        },
    }
}

impl LoggerComponentV1 {
    /// Create a file logger that writes to the specified path.
    ///
    /// The file is opened in append mode and created if it does not exist.
    /// ANSI color codes are never used for file output.
    /// Reads `RUST_LOG` for the log level threshold (default: info).
    ///
    /// # Errors
    ///
    /// Returns `io::Error` if the file cannot be opened or created.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use logger::LoggerComponentV1;
    /// use interfaces::ILogger;
    /// use component_core::query_interface;
    ///
    /// let component = LoggerComponentV1::new_with_file("/tmp/app.log").unwrap();
    /// let log = query_interface!(component, ILogger).unwrap();
    /// log.info("logging to file");
    /// ```
    pub fn new_with_file(path: &str) -> io::Result<Arc<Self>> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Self::new(LoggerState {
            writer: Mutex::new(Box::new(file)),
            level: LogLevel::from_env(),
            use_color: false,
        }))
    }

    /// Create a logger with an explicit writer, level, and color setting.
    ///
    /// Primarily used for testing.
    ///
    /// # Examples
    ///
    /// ```
    /// use logger::{LogLevel, LoggerComponentV1};
    /// use interfaces::ILogger;
    /// use component_core::query_interface;
    ///
    /// let buf: Vec<u8> = Vec::new();
    /// let component = LoggerComponentV1::new_with_writer(
    ///     Box::new(buf),
    ///     LogLevel::Debug,
    ///     false,
    /// );
    /// let log = query_interface!(component, ILogger).unwrap();
    /// log.info("captured to buffer");
    /// ```
    pub fn new_with_writer(
        writer: Box<dyn Write + Send>,
        level: LogLevel,
        use_color: bool,
    ) -> Arc<Self> {
        Self::new(LoggerState {
            writer: Mutex::new(writer),
            level,
            use_color,
        })
    }

    fn log(&self, level: LogLevel, msg: &str) {
        if level > self.state.level {
            return;
        }
        let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let line = if self.state.use_color {
            format!(
                "{} {}{}{} {}\n",
                timestamp,
                level.ansi_color(),
                level.as_str(),
                ANSI_RESET,
                msg
            )
        } else {
            format!("{} {} {}\n", timestamp, level.as_str(), msg)
        };
        let mut writer = self.state.writer.lock().unwrap();
        let _ = writer.write_all(line.as_bytes());
        let _ = writer.flush();
    }
}

impl ILogger for LoggerComponentV1 {
    fn error(&self, msg: &str) {
        self.log(LogLevel::Error, msg);
    }

    fn warn(&self, msg: &str) {
        self.log(LogLevel::Warn, msg);
    }

    fn info(&self, msg: &str) {
        self.log(LogLevel::Info, msg);
    }

    fn debug(&self, msg: &str) {
        self.log(LogLevel::Debug, msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_logger(
        level: LogLevel,
        use_color: bool,
    ) -> (Arc<LoggerComponentV1>, Arc<Mutex<Vec<u8>>>) {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer = TestWriter(Arc::clone(&buf));
        let comp = LoggerComponentV1::new_with_writer(Box::new(writer), level, use_color);
        (comp, buf)
    }

    #[derive(Clone)]
    struct TestWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for TestWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn test_log_level_ordering() {
        assert!(LogLevel::Error < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Debug);
    }

    #[test]
    fn test_log_level_from_env_str() {
        assert_eq!(LogLevel::from_env_str("error"), LogLevel::Error);
        assert_eq!(LogLevel::from_env_str("ERROR"), LogLevel::Error);
        assert_eq!(LogLevel::from_env_str("warn"), LogLevel::Warn);
        assert_eq!(LogLevel::from_env_str("warning"), LogLevel::Warn);
        assert_eq!(LogLevel::from_env_str("info"), LogLevel::Info);
        assert_eq!(LogLevel::from_env_str("INFO"), LogLevel::Info);
        assert_eq!(LogLevel::from_env_str("debug"), LogLevel::Debug);
        assert_eq!(LogLevel::from_env_str("trace"), LogLevel::Debug);
        assert_eq!(LogLevel::from_env_str("invalid"), LogLevel::Info);
        assert_eq!(LogLevel::from_env_str(""), LogLevel::Info);
    }

    #[test]
    fn test_log_level_display() {
        assert_eq!(format!("{}", LogLevel::Error), "ERROR");
        assert_eq!(format!("{}", LogLevel::Warn), "WARN ");
        assert_eq!(format!("{}", LogLevel::Info), "INFO ");
        assert_eq!(format!("{}", LogLevel::Debug), "DEBUG");
    }

    #[test]
    fn test_info_message_format() {
        let (logger, buf) = make_test_logger(LogLevel::Debug, false);
        logger.info("hello world");
        let output = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(output.contains("INFO "), "expected INFO in: {output}");
        assert!(
            output.contains("hello world"),
            "expected message in: {output}"
        );
        assert!(output.ends_with('\n'), "expected trailing newline");
        assert!(output.contains('T'), "expected ISO 8601 timestamp");
        assert!(output.contains('Z'), "expected UTC timezone marker");
    }

    #[test]
    fn test_error_message_format() {
        let (logger, buf) = make_test_logger(LogLevel::Debug, false);
        logger.error("disk failure");
        let output = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(output.contains("ERROR"), "expected ERROR in: {output}");
        assert!(output.contains("disk failure"));
    }

    #[test]
    fn test_warn_message_format() {
        let (logger, buf) = make_test_logger(LogLevel::Debug, false);
        logger.warn("high latency");
        let output = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(output.contains("WARN "));
        assert!(output.contains("high latency"));
    }

    #[test]
    fn test_debug_message_format() {
        let (logger, buf) = make_test_logger(LogLevel::Debug, false);
        logger.debug("cache hit");
        let output = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(output.contains("DEBUG"));
        assert!(output.contains("cache hit"));
    }

    #[test]
    fn test_level_filtering_suppresses_below_threshold() {
        let (logger, buf) = make_test_logger(LogLevel::Warn, false);
        logger.info("should not appear");
        logger.debug("should not appear either");
        let output = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(output.is_empty(), "expected no output, got: {output}");
    }

    #[test]
    fn test_level_filtering_allows_at_threshold() {
        let (logger, buf) = make_test_logger(LogLevel::Warn, false);
        logger.warn("should appear");
        logger.error("should also appear");
        let output = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(output.contains("should appear"));
        assert!(output.contains("should also appear"));
    }

    #[test]
    fn test_color_output_contains_ansi() {
        let (logger, buf) = make_test_logger(LogLevel::Debug, true);
        logger.error("red message");
        let output = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(output.contains("\x1b[31m"), "expected red ANSI code");
        assert!(output.contains("\x1b[0m"), "expected reset ANSI code");
    }

    #[test]
    fn test_no_color_output_no_ansi() {
        let (logger, buf) = make_test_logger(LogLevel::Debug, false);
        logger.error("plain message");
        let output = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(
            !output.contains("\x1b["),
            "unexpected ANSI code in: {output}"
        );
    }

    #[test]
    fn test_all_levels_colored() {
        let (logger, buf) = make_test_logger(LogLevel::Debug, true);
        logger.error("e");
        logger.warn("w");
        logger.info("i");
        logger.debug("d");
        let output = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(output.contains("\x1b[31m"), "missing red for error");
        assert!(output.contains("\x1b[33m"), "missing yellow for warn");
        assert!(output.contains("\x1b[32m"), "missing green for info");
        assert!(output.contains("\x1b[36m"), "missing cyan for debug");
    }

    #[test]
    fn test_file_output_no_ansi() {
        let dir = std::env::temp_dir().join("logger_test_file_output");
        let path = dir.to_str().unwrap();
        let _ = std::fs::remove_file(path);
        let component = LoggerComponentV1::new_with_file(path).unwrap();
        component.error("file error msg");
        component.warn("file warn msg");
        drop(component);
        let content = std::fs::read_to_string(path).unwrap();
        assert!(!content.contains("\x1b["), "ANSI codes in file output");
        assert!(content.contains("ERROR"));
        assert!(content.contains("file error msg"));
        assert!(content.contains("WARN "));
        assert!(content.contains("file warn msg"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_file_creation() {
        let dir = std::env::temp_dir().join("logger_test_file_creation");
        let path = dir.to_str().unwrap();
        let _ = std::fs::remove_file(path);
        assert!(!dir.exists());
        let component = LoggerComponentV1::new_with_file(path).unwrap();
        component.info("created");
        drop(component);
        assert!(dir.exists());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_file_error_on_invalid_path() {
        let result = LoggerComponentV1::new_with_file("/nonexistent/dir/file.log");
        assert!(result.is_err());
    }
}
