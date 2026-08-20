use std::any::Any;
use std::collections::HashMap;
use std::sync::RwLock;

use crate::component_ref::ComponentRef;
use crate::error::RegistryError;

/// A factory that produces component instances.
///
/// Implement this trait or use a closure via the blanket implementation.
/// The factory receives an optional type-erased configuration parameter.
///
/// # Examples
///
/// ```
/// use component_core::registry::ComponentFactory;
/// use component_core::component_ref::ComponentRef;
/// use component_core::error::RegistryError;
/// use std::any::Any;
///
/// // Factories are typically closures:
/// let factory = |_config: Option<&dyn Any>| -> Result<ComponentRef, RegistryError> {
///     Err(RegistryError::FactoryFailed {
///         name: "demo".to_string(),
///         source: "not implemented".to_string(),
///     })
/// };
/// ```
pub trait ComponentFactory: Send + Sync {
    /// Create a new component instance with optional configuration.
    fn create(&self, config: Option<&dyn Any>) -> Result<ComponentRef, RegistryError>;
}

impl<F> ComponentFactory for F
where
    F: Fn(Option<&dyn Any>) -> Result<ComponentRef, RegistryError> + Send + Sync,
{
    fn create(&self, config: Option<&dyn Any>) -> Result<ComponentRef, RegistryError> {
        (self)(config)
    }
}

/// A standalone component registry mapping string names to factories.
///
/// Multiple registries can coexist — there is no global state. All
/// operations are thread-safe via internal `RwLock`.
///
/// # Examples
///
/// ```
/// use component_core::registry::ComponentRegistry;
///
/// let registry = ComponentRegistry::new();
/// assert!(registry.list().is_empty());
/// ```
pub struct ComponentRegistry {
    factories: RwLock<HashMap<String, Box<dyn ComponentFactory>>>,
}

impl ComponentRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self {
            factories: RwLock::new(HashMap::new()),
        }
    }

    /// Registers a factory under the given name.
    ///
    /// Returns `Err(RegistryError::AlreadyRegistered)` if the name is taken.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::registry::ComponentRegistry;
    /// use component_core::component_ref::ComponentRef;
    /// use component_core::error::RegistryError;
    /// use std::any::Any;
    ///
    /// let registry = ComponentRegistry::new();
    /// registry.register("demo", |_: Option<&dyn Any>| -> Result<ComponentRef, RegistryError> {
    ///     Err(RegistryError::FactoryFailed {
    ///         name: "demo".into(), source: "stub".into()
    ///     })
    /// }).unwrap();
    ///
    /// assert!(registry.register("demo", |_: Option<&dyn Any>| -> Result<ComponentRef, RegistryError> {
    ///     Err(RegistryError::FactoryFailed {
    ///         name: "demo".into(), source: "stub".into()
    ///     })
    /// }).is_err());
    /// ```
    pub fn register<F: ComponentFactory + 'static>(
        &self,
        name: &str,
        factory: F,
    ) -> Result<(), RegistryError> {
        let mut factories = self.factories.write().unwrap();
        if factories.contains_key(name) {
            return Err(RegistryError::AlreadyRegistered {
                name: name.to_string(),
            });
        }
        factories.insert(name.to_string(), Box::new(factory));
        Ok(())
    }

    /// Registers a simple no-config factory under the given name.
    ///
    /// The factory closure takes no arguments and returns an
    /// `Arc<dyn IUnknown>`. This is a convenience wrapper for the common
    /// case where the factory ignores the configuration parameter.
    ///
    /// # Errors
    ///
    /// Returns `Err(RegistryError::AlreadyRegistered)` if the name is taken.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::registry::ComponentRegistry;
    /// use component_core::component_ref::ComponentRef;
    /// use component_core::iunknown::IUnknown;
    /// use component_core::interface::{InterfaceInfo, ReceptacleInfo};
    /// use component_core::error::RegistryError;
    /// use std::any::{Any, TypeId};
    /// use std::sync::Arc;
    ///
    /// struct Dummy;
    /// impl IUnknown for Dummy {
    ///     fn query_interface_raw(&self, _id: TypeId) -> Option<&(dyn Any + Send + Sync)> { None }
    ///     fn version(&self) -> &str { "1.0.0" }
    ///     fn provided_interfaces(&self) -> &[InterfaceInfo] { &[] }
    ///     fn receptacles(&self) -> &[ReceptacleInfo] { &[] }
    ///     fn connect_receptacle_raw(&self, _: &str, _: &dyn IUnknown)
    ///         -> Result<(), RegistryError> {
    ///         Err(RegistryError::BindingFailed { detail: "none".into() })
    ///     }
    /// }
    ///
    /// let registry = ComponentRegistry::new();
    /// registry.register_simple("demo", || ComponentRef::from(Arc::new(Dummy))).unwrap();
    ///
    /// let comp = registry.create("demo", None).unwrap();
    /// assert_eq!(comp.version(), "1.0.0");
    /// ```
    pub fn register_simple<F>(&self, name: &str, factory: F) -> Result<(), RegistryError>
    where
        F: Fn() -> ComponentRef + Send + Sync + 'static,
    {
        self.register(name, move |_config: Option<&dyn Any>| Ok(factory()))
    }

    /// Removes a factory by name.
    ///
    /// Returns `Err(RegistryError::NotFound)` if the name is not registered.
    pub fn unregister(&self, name: &str) -> Result<(), RegistryError> {
        let mut factories = self.factories.write().unwrap();
        if factories.remove(name).is_none() {
            return Err(RegistryError::NotFound {
                name: name.to_string(),
            });
        }
        Ok(())
    }

    /// Creates a component by looking up the named factory.
    ///
    /// Passes the optional `config` to the factory. Factory panics are
    /// caught and converted to `RegistryError::FactoryFailed`.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::registry::ComponentRegistry;
    /// use component_core::error::RegistryError;
    ///
    /// let registry = ComponentRegistry::new();
    /// let result = registry.create("missing", None);
    /// assert!(matches!(result, Err(RegistryError::NotFound { .. })));
    /// ```
    pub fn create(
        &self,
        name: &str,
        config: Option<&dyn Any>,
    ) -> Result<ComponentRef, RegistryError> {
        let factories = self.factories.read().unwrap();
        let factory = factories.get(name).ok_or_else(|| RegistryError::NotFound {
            name: name.to_string(),
        })?;

        // Catch panics from the factory
        let name_owned = name.to_string();
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| factory.create(config)));

        match result {
            Ok(Ok(component_ref)) => Ok(component_ref),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(RegistryError::FactoryFailed {
                name: name_owned,
                source: "factory panicked".to_string(),
            }),
        }
    }

    /// Lists all registered component names.
    ///
    /// The order is not guaranteed.
    pub fn list(&self) -> Vec<String> {
        let factories = self.factories.read().unwrap();
        factories.keys().cloned().collect()
    }
}

impl Default for ComponentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component_ref::ComponentRef;
    use crate::interface::{InterfaceInfo, ReceptacleInfo};
    use crate::iunknown::IUnknown;
    use std::any::{Any, TypeId};
    use std::sync::Arc;

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
        fn connect_receptacle_raw(&self, _: &str, _: &dyn IUnknown) -> Result<(), RegistryError> {
            Err(RegistryError::BindingFailed {
                detail: "none".into(),
            })
        }
    }

    fn dummy_factory(_config: Option<&dyn Any>) -> Result<ComponentRef, RegistryError> {
        Ok(ComponentRef::from(Arc::new(Dummy)))
    }

    #[test]
    fn new_creates_empty_registry() {
        let reg = ComponentRegistry::new();
        assert!(reg.list().is_empty());
    }

    #[test]
    fn register_and_create_lifecycle() {
        let reg = ComponentRegistry::new();
        reg.register("dummy", dummy_factory).unwrap();
        let comp = reg.create("dummy", None).unwrap();
        assert_eq!(comp.version(), "1.0.0");
    }

    #[test]
    fn create_returns_not_found_for_unregistered() {
        let reg = ComponentRegistry::new();
        let err = reg.create("missing", None).unwrap_err();
        assert!(matches!(err, RegistryError::NotFound { .. }));
    }

    #[test]
    fn register_returns_already_registered_for_duplicate() {
        let reg = ComponentRegistry::new();
        reg.register("dummy", dummy_factory).unwrap();
        let err = reg.register("dummy", dummy_factory).unwrap_err();
        assert!(matches!(err, RegistryError::AlreadyRegistered { .. }));
    }

    #[test]
    fn list_returns_all_registered_names() {
        let reg = ComponentRegistry::new();
        reg.register("alpha", dummy_factory).unwrap();
        reg.register("beta", dummy_factory).unwrap();
        let mut names = reg.list();
        names.sort();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn unregister_removes_factory() {
        let reg = ComponentRegistry::new();
        reg.register("dummy", dummy_factory).unwrap();
        reg.unregister("dummy").unwrap();
        assert!(reg.list().is_empty());
        assert!(matches!(
            reg.create("dummy", None),
            Err(RegistryError::NotFound { .. })
        ));
    }

    #[test]
    fn unregister_returns_not_found_for_missing() {
        let reg = ComponentRegistry::new();
        let err = reg.unregister("missing").unwrap_err();
        assert!(matches!(err, RegistryError::NotFound { .. }));
    }

    #[test]
    fn create_with_typed_config() {
        let reg = ComponentRegistry::new();
        reg.register(
            "configured",
            |config: Option<&dyn Any>| -> Result<ComponentRef, RegistryError> {
                let val = config.unwrap().downcast_ref::<u32>().unwrap();
                assert_eq!(*val, 42);
                Ok(ComponentRef::from(Arc::new(Dummy)))
            },
        )
        .unwrap();
        reg.create("configured", Some(&42u32)).unwrap();
    }

    #[test]
    fn registry_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ComponentRegistry>();
    }

    #[test]
    fn factory_panic_is_caught() {
        let reg = ComponentRegistry::new();
        reg.register(
            "panicker",
            |_: Option<&dyn Any>| -> Result<ComponentRef, RegistryError> {
                panic!("oops");
            },
        )
        .unwrap();
        let err = reg.create("panicker", None).unwrap_err();
        assert!(matches!(err, RegistryError::FactoryFailed { .. }));
    }
}
