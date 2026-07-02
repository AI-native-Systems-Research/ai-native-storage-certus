use std::fmt;

/// Unique identifier for a remote zyre peer (UUID string).
///
/// # Example
///
/// ```
/// use zyre::PeerId;
///
/// let id = PeerId::from("abc-123");
/// assert_eq!(id.as_str(), "abc-123");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PeerId(String);

impl PeerId {
    /// Create a PeerId from a UUID string.
    pub fn new(uuid: impl Into<String>) -> Self {
        Self(uuid.into())
    }

    /// Return the UUID string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for PeerId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for PeerId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn peer_id_display() {
        let id = PeerId::new("abc123");
        assert_eq!(id.to_string(), "abc123");
        assert_eq!(id.as_str(), "abc123");
    }

    #[test]
    fn peer_id_equality() {
        let a = PeerId::new("same");
        let b = PeerId::new("same");
        let c = PeerId::new("different");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn peer_id_hashable() {
        let mut set = HashSet::new();
        set.insert(PeerId::new("one"));
        set.insert(PeerId::new("two"));
        set.insert(PeerId::new("one"));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn peer_id_from_conversions() {
        let from_str: PeerId = "hello".into();
        let from_string: PeerId = String::from("hello").into();
        assert_eq!(from_str, from_string);
    }
}
