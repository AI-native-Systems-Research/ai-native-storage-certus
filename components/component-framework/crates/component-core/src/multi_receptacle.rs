use std::sync::{Arc, RwLock};

/// A multi-slot receptacle that accepts multiple providers of the same interface.
///
/// Unlike [`Receptacle`](crate::receptacle::Receptacle) which holds exactly one
/// provider, `MultiReceptacle` accumulates providers via [`push`](Self::push).
/// This is used for components that require N instances of the same interface
/// (e.g., a dispatcher receiving multiple block-device drives).
///
/// Thread-safe: uses `RwLock` internally.
///
/// # Examples
///
/// ```
/// use component_core::multi_receptacle::MultiReceptacle;
/// use std::sync::Arc;
///
/// trait IBlockDevice: Send + Sync {
///     fn id(&self) -> u32;
/// }
///
/// struct Drive(u32);
/// impl IBlockDevice for Drive {
///     fn id(&self) -> u32 { self.0 }
/// }
///
/// let mr: MultiReceptacle<dyn IBlockDevice + Send + Sync> = MultiReceptacle::new();
/// assert!(mr.is_empty());
///
/// mr.push(Arc::new(Drive(0)) as Arc<dyn IBlockDevice + Send + Sync>);
/// mr.push(Arc::new(Drive(1)) as Arc<dyn IBlockDevice + Send + Sync>);
/// assert_eq!(mr.len(), 2);
///
/// let all = mr.get_all();
/// assert_eq!(all[0].id(), 0);
/// assert_eq!(all[1].id(), 1);
/// ```
pub struct MultiReceptacle<T: ?Sized + Send + Sync + 'static> {
    connections: RwLock<Vec<Arc<T>>>,
}

impl<T: ?Sized + Send + Sync + 'static> MultiReceptacle<T> {
    /// Creates a new empty multi-receptacle.
    pub fn new() -> Self {
        Self {
            connections: RwLock::new(Vec::new()),
        }
    }

    /// Appends a provider to this multi-receptacle.
    pub fn push(&self, provider: Arc<T>) {
        let mut guard = self.connections.write().unwrap();
        guard.push(provider);
    }

    /// Returns clones of all connected providers.
    pub fn get_all(&self) -> Vec<Arc<T>> {
        let guard = self.connections.read().unwrap();
        guard.clone()
    }

    /// Returns the number of connected providers.
    pub fn len(&self) -> usize {
        self.connections.read().unwrap().len()
    }

    /// Returns `true` if no providers are connected.
    pub fn is_empty(&self) -> bool {
        self.connections.read().unwrap().is_empty()
    }
}

impl<T: ?Sized + Send + Sync + 'static> Default for MultiReceptacle<T> {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: RwLock<Vec<Arc<T>>> is Send + Sync when T: Send + Sync.
unsafe impl<T: ?Sized + Send + Sync + 'static> Send for MultiReceptacle<T> {}
unsafe impl<T: ?Sized + Send + Sync + 'static> Sync for MultiReceptacle<T> {}

#[cfg(test)]
mod tests {
    use super::*;

    trait IService: Send + Sync {
        fn value(&self) -> u32;
    }

    struct Impl(u32);
    impl IService for Impl {
        fn value(&self) -> u32 {
            self.0
        }
    }

    #[test]
    fn new_creates_empty() {
        let mr: MultiReceptacle<dyn IService + Send + Sync> = MultiReceptacle::new();
        assert!(mr.is_empty());
        assert_eq!(mr.len(), 0);
    }

    #[test]
    fn push_and_get_all() {
        let mr: MultiReceptacle<dyn IService + Send + Sync> = MultiReceptacle::new();
        mr.push(Arc::new(Impl(10)) as Arc<dyn IService + Send + Sync>);
        mr.push(Arc::new(Impl(20)) as Arc<dyn IService + Send + Sync>);

        let all = mr.get_all();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].value(), 10);
        assert_eq!(all[1].value(), 20);
    }

    #[test]
    fn len_tracks_push() {
        let mr: MultiReceptacle<dyn IService + Send + Sync> = MultiReceptacle::new();
        assert_eq!(mr.len(), 0);
        mr.push(Arc::new(Impl(1)) as Arc<dyn IService + Send + Sync>);
        assert_eq!(mr.len(), 1);
        mr.push(Arc::new(Impl(2)) as Arc<dyn IService + Send + Sync>);
        assert_eq!(mr.len(), 2);
    }
}
