//! The crate's error type.
//!
//! One type, carrying a message. The requirements this crate implements are
//! specific about what an error must *say* — FR-004's refusal has to name both
//! the requested and the effective mean, for instance — so the useful detail is
//! in the message rather than in a variant tag. A caller's only reasonable
//! response to any of these is to print it and stop: they are all defects in a
//! description file, found before a single operation is issued (FR-002).

use std::fmt;

/// An error in a workload description, or in a distribution derived from one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    message: String,
}

impl Error {
    /// Build an error from anything displayable.
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_model::Error;
    ///
    /// let e = Error::new("pool size 0 is not usable");
    /// assert!(e.to_string().contains("pool size"));
    /// ```
    pub fn new(message: impl fmt::Display) -> Self {
        Self {
            message: message.to_string(),
        }
    }

    /// Prefix the message with the field or path the error was found in.
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_model::Error;
    ///
    /// let e = Error::new("sigma must be positive").within("shared_classes.tool.lifetime");
    /// assert_eq!(
    ///     e.to_string(),
    ///     "shared_classes.tool.lifetime: sigma must be positive"
    /// );
    /// ```
    pub fn within(self, context: impl fmt::Display) -> Self {
        Self {
            message: format!("{context}: {}", self.message),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// Shorthand for this crate's fallible operations.
pub type Result<T> = std::result::Result<T, Error>;
