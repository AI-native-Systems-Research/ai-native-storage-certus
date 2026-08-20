use std::sync::{Arc, RwLock};

use crate::error::ReceptacleError;

/// A typed slot representing a required interface.
///
/// Each receptacle connects to exactly one provider at a time. The
/// provider must be disconnected before a new one can be connected.
///
/// Thread-safe: uses `RwLock` internally so multiple threads can
/// call [`get`](Receptacle::get) concurrently while
/// [`connect`](Receptacle::connect) and [`disconnect`](Receptacle::disconnect)
/// take exclusive access.
///
/// # Examples
///
/// ```
/// use component_core::receptacle::Receptacle;
/// use std::sync::Arc;
///
/// trait ILogger: Send + Sync {
///     fn log(&self, msg: &str);
/// }
///
/// struct ConsoleLogger;
/// impl ILogger for ConsoleLogger {
///     fn log(&self, msg: &str) { println!("{msg}"); }
/// }
///
/// let receptacle: Receptacle<dyn ILogger + Send + Sync> = Receptacle::new();
/// assert!(!receptacle.is_connected());
///
/// let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(ConsoleLogger);
/// receptacle.connect(logger).unwrap();
/// assert!(receptacle.is_connected());
///
/// let provider = receptacle.get().unwrap();
/// provider.log("hello");
///
/// receptacle.disconnect().unwrap();
/// assert!(!receptacle.is_connected());
/// ```
pub struct Receptacle<T: ?Sized + Send + Sync + 'static> {
    connection: RwLock<Option<Arc<T>>>,
}

impl<T: ?Sized + Send + Sync + 'static> Receptacle<T> {
    /// Creates a new disconnected receptacle.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::receptacle::Receptacle;
    ///
    /// let r: Receptacle<dyn Send + Sync> = Receptacle::new();
    /// assert!(!r.is_connected());
    /// ```
    pub fn new() -> Self {
        Self {
            connection: RwLock::new(None),
        }
    }

    /// Connects a provider to this receptacle.
    ///
    /// Returns `Err(ReceptacleError::AlreadyConnected)` if the
    /// receptacle already has a connection. Call [`disconnect`](Self::disconnect)
    /// first.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::receptacle::Receptacle;
    /// use component_core::error::ReceptacleError;
    /// use std::sync::Arc;
    ///
    /// let r: Receptacle<dyn Send + Sync> = Receptacle::new();
    /// let provider: Arc<dyn Send + Sync> = Arc::new(42u32);
    /// assert!(r.connect(provider.clone()).is_ok());
    ///
    /// // Second connect fails
    /// assert_eq!(r.connect(provider), Err(ReceptacleError::AlreadyConnected));
    /// ```
    pub fn connect(&self, provider: Arc<T>) -> Result<(), ReceptacleError> {
        let mut guard = self.connection.write().unwrap();
        if guard.is_some() {
            return Err(ReceptacleError::AlreadyConnected);
        }
        *guard = Some(provider);
        Ok(())
    }

    /// Disconnects the current provider.
    ///
    /// Returns `Err(ReceptacleError::NotConnected)` if no provider is connected.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::receptacle::Receptacle;
    /// use component_core::error::ReceptacleError;
    /// use std::sync::Arc;
    ///
    /// let r: Receptacle<dyn Send + Sync> = Receptacle::new();
    /// assert_eq!(r.disconnect(), Err(ReceptacleError::NotConnected));
    ///
    /// r.connect(Arc::new(42u32) as Arc<dyn Send + Sync>).unwrap();
    /// assert!(r.disconnect().is_ok());
    /// ```
    pub fn disconnect(&self) -> Result<(), ReceptacleError> {
        let mut guard = self.connection.write().unwrap();
        if guard.is_none() {
            return Err(ReceptacleError::NotConnected);
        }
        *guard = None;
        Ok(())
    }

    /// Returns `true` if a provider is currently connected.
    pub fn is_connected(&self) -> bool {
        self.connection.read().unwrap().is_some()
    }

    /// Returns a clone of the connected provider's `Arc`.
    ///
    /// Returns `Err(ReceptacleError::NotConnected)` if no provider is connected.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::receptacle::Receptacle;
    /// use component_core::error::ReceptacleError;
    /// use std::sync::Arc;
    ///
    /// let r: Receptacle<dyn Send + Sync> = Receptacle::new();
    /// assert!(matches!(r.get(), Err(ReceptacleError::NotConnected)));
    ///
    /// r.connect(Arc::new(42u32) as Arc<dyn Send + Sync>).unwrap();
    /// let provider = r.get().unwrap();
    /// ```
    pub fn get(&self) -> Result<Arc<T>, ReceptacleError> {
        let guard = self.connection.read().unwrap();
        guard
            .as_ref()
            .map(Arc::clone)
            .ok_or(ReceptacleError::NotConnected)
    }
}

impl<T: ?Sized + Send + Sync + 'static> Default for Receptacle<T> {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: RwLock<Option<Arc<T>>> is Send + Sync when T: Send + Sync.
// The bounds on T already enforce this.
unsafe impl<T: ?Sized + Send + Sync + 'static> Send for Receptacle<T> {}
unsafe impl<T: ?Sized + Send + Sync + 'static> Sync for Receptacle<T> {}

#[cfg(test)]
mod tests {
    use super::*;

    trait ITestService: Send + Sync {
        fn call(&self) -> &str;
    }

    struct TestImpl;
    impl ITestService for TestImpl {
        fn call(&self) -> &str {
            "test"
        }
    }

    #[test]
    fn new_creates_disconnected() {
        let r: Receptacle<dyn ITestService + Send + Sync> = Receptacle::new();
        assert!(!r.is_connected());
    }

    #[test]
    fn connect_disconnect_lifecycle() {
        let r: Receptacle<dyn ITestService + Send + Sync> = Receptacle::new();
        let provider: Arc<dyn ITestService + Send + Sync> = Arc::new(TestImpl);

        r.connect(provider).unwrap();
        assert!(r.is_connected());

        let p = r.get().unwrap();
        assert_eq!(p.call(), "test");

        r.disconnect().unwrap();
        assert!(!r.is_connected());
    }

    #[test]
    fn connect_already_connected_returns_error() {
        let r: Receptacle<dyn ITestService + Send + Sync> = Receptacle::new();
        let p1: Arc<dyn ITestService + Send + Sync> = Arc::new(TestImpl);
        let p2: Arc<dyn ITestService + Send + Sync> = Arc::new(TestImpl);

        r.connect(p1).unwrap();
        assert_eq!(r.connect(p2), Err(ReceptacleError::AlreadyConnected));
    }

    #[test]
    fn get_disconnected_returns_error() {
        let r: Receptacle<dyn ITestService + Send + Sync> = Receptacle::new();
        assert!(matches!(r.get(), Err(ReceptacleError::NotConnected)));
    }

    #[test]
    fn disconnect_when_not_connected_returns_error() {
        let r: Receptacle<dyn ITestService + Send + Sync> = Receptacle::new();
        assert_eq!(r.disconnect(), Err(ReceptacleError::NotConnected));
    }

    #[test]
    fn receptacle_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Receptacle<dyn ITestService + Send + Sync>>();
    }

    #[test]
    fn reconnect_after_disconnect() {
        let r: Receptacle<dyn ITestService + Send + Sync> = Receptacle::new();
        let p: Arc<dyn ITestService + Send + Sync> = Arc::new(TestImpl);

        r.connect(p.clone()).unwrap();
        r.disconnect().unwrap();
        r.connect(p).unwrap();
        assert!(r.is_connected());
    }
}
