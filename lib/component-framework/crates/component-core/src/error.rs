use std::fmt;

/// Errors from receptacle operations.
///
/// # Examples
///
/// ```
/// use component_core::error::ReceptacleError;
///
/// let err = ReceptacleError::NotConnected;
/// assert_eq!(format!("{err}"), "receptacle is not connected");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceptacleError {
    /// Attempted to invoke or disconnect a receptacle with no connection.
    NotConnected,
    /// Attempted to connect a receptacle that already has a connection.
    AlreadyConnected,
}

impl fmt::Display for ReceptacleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConnected => write!(f, "receptacle is not connected"),
            Self::AlreadyConnected => write!(f, "receptacle is already connected"),
        }
    }
}

impl std::error::Error for ReceptacleError {}

/// Errors from interface queries.
///
/// # Examples
///
/// ```
/// use component_core::error::QueryError;
///
/// let err = QueryError::InterfaceNotFound;
/// assert_eq!(format!("{err}"), "interface not found on this component");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    /// The requested interface is not provided by this component.
    InterfaceNotFound,
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InterfaceNotFound => write!(f, "interface not found on this component"),
        }
    }
}

impl std::error::Error for QueryError {}

/// Errors from registry and binding operations.
///
/// # Examples
///
/// ```
/// use component_core::error::RegistryError;
///
/// let err = RegistryError::NotFound { name: "MyComponent".to_string() };
/// assert_eq!(format!("{err}"), "component not found: MyComponent");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// Requested component name is not registered.
    NotFound {
        /// The name that was looked up.
        name: String,
    },
    /// A factory is already registered under this name.
    AlreadyRegistered {
        /// The duplicate name.
        name: String,
    },
    /// The factory failed during component creation.
    FactoryFailed {
        /// The component name whose factory failed.
        name: String,
        /// Description of the failure.
        source: String,
    },
    /// Third-party binding failed.
    BindingFailed {
        /// Description of the binding failure.
        detail: String,
    },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { name } => write!(f, "component not found: {name}"),
            Self::AlreadyRegistered { name } => {
                write!(f, "component already registered: {name}")
            }
            Self::FactoryFailed { name, source } => {
                write!(f, "factory failed for {name}: {source}")
            }
            Self::BindingFailed { detail } => write!(f, "binding failed: {detail}"),
        }
    }
}

impl std::error::Error for RegistryError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receptacle_error_display() {
        assert_eq!(
            ReceptacleError::NotConnected.to_string(),
            "receptacle is not connected"
        );
        assert_eq!(
            ReceptacleError::AlreadyConnected.to_string(),
            "receptacle is already connected"
        );
    }

    #[test]
    fn query_error_display() {
        assert_eq!(
            QueryError::InterfaceNotFound.to_string(),
            "interface not found on this component"
        );
    }

    #[test]
    fn errors_implement_std_error() {
        fn assert_error<E: std::error::Error>() {}
        assert_error::<ReceptacleError>();
        assert_error::<QueryError>();
    }

    #[test]
    fn errors_are_eq() {
        assert_eq!(ReceptacleError::NotConnected, ReceptacleError::NotConnected);
        assert_ne!(
            ReceptacleError::NotConnected,
            ReceptacleError::AlreadyConnected
        );
    }

    #[test]
    fn registry_error_display() {
        assert_eq!(
            RegistryError::NotFound {
                name: "Foo".to_string()
            }
            .to_string(),
            "component not found: Foo"
        );
        assert_eq!(
            RegistryError::AlreadyRegistered {
                name: "Foo".to_string()
            }
            .to_string(),
            "component already registered: Foo"
        );
        assert_eq!(
            RegistryError::FactoryFailed {
                name: "Foo".to_string(),
                source: "panic".to_string()
            }
            .to_string(),
            "factory failed for Foo: panic"
        );
        assert_eq!(
            RegistryError::BindingFailed {
                detail: "type mismatch".to_string()
            }
            .to_string(),
            "binding failed: type mismatch"
        );
    }

    #[test]
    fn registry_error_implements_std_error() {
        fn assert_error<E: std::error::Error>() {}
        assert_error::<RegistryError>();
    }
}
