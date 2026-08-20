use std::ops::Deref;
use std::sync::Arc;

use crate::iunknown::IUnknown;

/// A reference-counted handle to a component instance.
///
/// Wraps `Arc<dyn IUnknown>` internally. `attach()` clones the Arc
/// (incrementing the reference count); dropping the handle releases it
/// (decrementing the count). When the count reaches zero, the component
/// is destroyed.
///
/// Rust's ownership system prevents use-after-free at compile time.
///
/// # Examples
///
/// ```
/// use component_core::iunknown::IUnknown;
/// use component_core::interface::{InterfaceInfo, ReceptacleInfo};
/// use component_core::component_ref::ComponentRef;
/// use std::any::{Any, TypeId};
/// use std::sync::Arc;
///
/// struct Demo;
/// impl IUnknown for Demo {
///     fn query_interface_raw(&self, _id: TypeId) -> Option<&(dyn Any + Send + Sync)> { None }
///     fn version(&self) -> &str { "1.0.0" }
///     fn provided_interfaces(&self) -> &[InterfaceInfo] { &[] }
///     fn receptacles(&self) -> &[ReceptacleInfo] { &[] }
///     fn connect_receptacle_raw(&self, _name: &str, _provider: &dyn component_core::iunknown::IUnknown) -> Result<(), component_core::error::RegistryError> {
///         Err(component_core::error::RegistryError::BindingFailed { detail: "no receptacles".to_string() })
///     }
/// }
///
/// let comp = ComponentRef::new(Arc::new(Demo) as Arc<dyn IUnknown>);
/// assert_eq!(comp.version(), "1.0.0");
///
/// let comp2 = comp.attach();
/// assert_eq!(comp2.version(), "1.0.0");
/// ```
pub struct ComponentRef {
    inner: Arc<dyn IUnknown>,
}

impl std::fmt::Debug for ComponentRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentRef")
            .field("version", &self.inner.version())
            .field("ref_count", &Arc::strong_count(&self.inner))
            .finish()
    }
}

impl ComponentRef {
    /// Creates a new `ComponentRef` from an `Arc<dyn IUnknown>`.
    pub fn new(inner: Arc<dyn IUnknown>) -> Self {
        Self { inner }
    }

    /// Creates a new handle to the same component (increments reference count).
    ///
    /// Equivalent to `Clone::clone`.
    pub fn attach(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Returns the current strong reference count.
    ///
    /// Useful for testing and debugging. The count includes this handle.
    pub fn ref_count(&self) -> usize {
        Arc::strong_count(&self.inner)
    }
}

impl Clone for ComponentRef {
    fn clone(&self) -> Self {
        self.attach()
    }
}

impl Deref for ComponentRef {
    type Target = dyn IUnknown;

    fn deref(&self) -> &Self::Target {
        &*self.inner
    }
}

impl<T: IUnknown + 'static> From<Arc<T>> for ComponentRef {
    fn from(arc: Arc<T>) -> Self {
        Self {
            inner: arc as Arc<dyn IUnknown>,
        }
    }
}

// SAFETY: Arc<dyn IUnknown> is Send + Sync because IUnknown: Send + Sync.
unsafe impl Send for ComponentRef {}
unsafe impl Sync for ComponentRef {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::RegistryError;
    use crate::interface::{InterfaceInfo, ReceptacleInfo};
    use std::any::{Any, TypeId};
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Dummy;
    impl IUnknown for Dummy {
        fn query_interface_raw(&self, _id: TypeId) -> Option<&(dyn Any + Send + Sync)> {
            None
        }
        fn version(&self) -> &str {
            "1.0.0"
        }
        fn provided_interfaces(&self) -> &[InterfaceInfo] {
            &[]
        }
        fn receptacles(&self) -> &[ReceptacleInfo] {
            &[]
        }
        fn connect_receptacle_raw(
            &self,
            _name: &str,
            _provider: &dyn IUnknown,
        ) -> Result<(), RegistryError> {
            Err(RegistryError::BindingFailed {
                detail: "none".into(),
            })
        }
    }

    #[test]
    fn wraps_arc_and_provides_iunknown_access() {
        let comp = ComponentRef::new(Arc::new(Dummy) as Arc<dyn IUnknown>);
        assert_eq!(comp.version(), "1.0.0");
    }

    #[test]
    fn attach_clones_handle_and_increments_count() {
        let comp = ComponentRef::new(Arc::new(Dummy) as Arc<dyn IUnknown>);
        assert_eq!(comp.ref_count(), 1);
        let comp2 = comp.attach();
        assert_eq!(comp.ref_count(), 2);
        assert_eq!(comp2.ref_count(), 2);
    }

    #[test]
    fn drop_decrements_count_and_destroys_at_zero() {
        static DROPPED: AtomicBool = AtomicBool::new(false);

        struct Tracked;
        impl Drop for Tracked {
            fn drop(&mut self) {
                DROPPED.store(true, Ordering::SeqCst);
            }
        }
        impl IUnknown for Tracked {
            fn query_interface_raw(&self, _id: TypeId) -> Option<&(dyn Any + Send + Sync)> {
                None
            }
            fn version(&self) -> &str {
                "1.0.0"
            }
            fn provided_interfaces(&self) -> &[InterfaceInfo] {
                &[]
            }
            fn receptacles(&self) -> &[ReceptacleInfo] {
                &[]
            }
            fn connect_receptacle_raw(
                &self,
                _: &str,
                _: &dyn IUnknown,
            ) -> Result<(), RegistryError> {
                Err(RegistryError::BindingFailed {
                    detail: "none".into(),
                })
            }
        }

        DROPPED.store(false, Ordering::SeqCst);
        let comp = ComponentRef::new(Arc::new(Tracked) as Arc<dyn IUnknown>);
        let comp2 = comp.attach();
        assert_eq!(comp.ref_count(), 2);
        drop(comp);
        assert_eq!(comp2.ref_count(), 1);
        assert!(!DROPPED.load(Ordering::SeqCst));
        drop(comp2);
        assert!(DROPPED.load(Ordering::SeqCst));
    }

    #[test]
    fn is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ComponentRef>();
    }

    #[test]
    fn clone_is_same_as_attach() {
        let comp = ComponentRef::new(Arc::new(Dummy) as Arc<dyn IUnknown>);
        let cloned = comp.clone();
        assert_eq!(comp.ref_count(), 2);
        assert_eq!(cloned.ref_count(), 2);
    }

    #[test]
    fn from_arc_t() {
        let arc = Arc::new(Dummy);
        let comp: ComponentRef = ComponentRef::from(arc);
        assert_eq!(comp.version(), "1.0.0");
        assert_eq!(comp.ref_count(), 1);
    }
}
