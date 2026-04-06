use crate::error::RegistryError;
use crate::iunknown::IUnknown;

/// Wire two components using string-based interface and receptacle names.
///
/// This is the third-party binding mechanism: the caller does not need
/// compile-time knowledge of the concrete component types. The function
/// resolves the interface by name from the provider's metadata, verifies
/// type compatibility with the consumer's receptacle, then delegates to
/// `connect_receptacle_raw` which queries the provider internally.
///
/// # Errors
///
/// Returns `RegistryError::BindingFailed` if:
/// - The provider does not have an interface with the given name
/// - The consumer does not have a receptacle with the given name
/// - The interface and receptacle types do not match
/// - The receptacle is already connected
///
/// # Examples
///
/// ```
/// use component_core::binding::bind;
/// use component_core::iunknown::{IUnknown, query};
/// use component_core::interface::{InterfaceInfo, ReceptacleInfo};
/// use component_core::receptacle::Receptacle;
/// use component_core::error::RegistryError;
/// use std::any::{Any, TypeId};
/// use std::sync::Arc;
///
/// // Define a simple interface trait
/// trait IGreeter: Send + Sync {
///     fn greet(&self) -> &str;
/// }
///
/// // A provider component that implements IGreeter
/// struct Provider {
///     greeter: Arc<dyn IGreeter + Send + Sync>,
/// }
///
/// impl IUnknown for Provider {
///     fn query_interface_raw(&self, id: TypeId) -> Option<&(dyn Any + Send + Sync)> {
///         if id == TypeId::of::<Arc<dyn IGreeter + Send + Sync>>() {
///             Some(&self.greeter)
///         } else { None }
///     }
///     fn version(&self) -> &str { "1.0.0" }
///     fn provided_interfaces(&self) -> &[InterfaceInfo] {
///         static INFO: std::sync::OnceLock<Vec<InterfaceInfo>> = std::sync::OnceLock::new();
///         INFO.get_or_init(|| vec![InterfaceInfo {
///             type_id: TypeId::of::<Arc<dyn IGreeter + Send + Sync>>(),
///             name: "IGreeter",
///         }])
///     }
///     fn receptacles(&self) -> &[ReceptacleInfo] { &[] }
///     fn connect_receptacle_raw(&self, _: &str, _: &dyn IUnknown) -> Result<(), RegistryError> {
///         Err(RegistryError::BindingFailed { detail: "none".into() })
///     }
/// }
///
/// // A consumer component with a receptacle for IGreeter
/// struct Consumer {
///     greeter: Receptacle<dyn IGreeter + Send + Sync>,
/// }
///
/// impl IUnknown for Consumer {
///     fn query_interface_raw(&self, _: TypeId) -> Option<&(dyn Any + Send + Sync)> { None }
///     fn version(&self) -> &str { "1.0.0" }
///     fn provided_interfaces(&self) -> &[InterfaceInfo] { &[] }
///     fn receptacles(&self) -> &[ReceptacleInfo] {
///         static INFO: std::sync::OnceLock<Vec<ReceptacleInfo>> = std::sync::OnceLock::new();
///         INFO.get_or_init(|| vec![ReceptacleInfo {
///             type_id: TypeId::of::<Arc<dyn IGreeter + Send + Sync>>(),
///             name: "greeter",
///             interface_name: "IGreeter",
///         }])
///     }
///     fn connect_receptacle_raw(&self, name: &str, provider: &dyn IUnknown) -> Result<(), RegistryError> {
///         if name == "greeter" {
///             let arc = query::<dyn IGreeter + Send + Sync>(provider)
///                 .ok_or(RegistryError::BindingFailed { detail: "no IGreeter".into() })?;
///             self.greeter.connect(arc).map_err(|e| RegistryError::BindingFailed {
///                 detail: e.to_string(),
///             })
///         } else {
///             Err(RegistryError::BindingFailed { detail: "unknown".into() })
///         }
///     }
/// }
///
/// struct MyGreeter;
/// impl IGreeter for MyGreeter { fn greet(&self) -> &str { "hi" } }
///
/// let provider = Provider { greeter: Arc::new(MyGreeter) };
/// let consumer = Consumer { greeter: Receptacle::new() };
///
/// bind(&provider, "IGreeter", &consumer, "greeter").unwrap();
/// assert_eq!(consumer.greeter.get().unwrap().greet(), "hi");
/// ```
pub fn bind(
    provider: &dyn IUnknown,
    interface_name: &str,
    consumer: &dyn IUnknown,
    receptacle_name: &str,
) -> Result<(), RegistryError> {
    // 1. Find the interface TypeId by name from provider metadata
    let iface_info = provider
        .provided_interfaces()
        .iter()
        .find(|info| info.name == interface_name)
        .ok_or_else(|| RegistryError::BindingFailed {
            detail: format!("provider does not have interface '{}'", interface_name),
        })?;

    // 2. Find the receptacle by name from consumer metadata
    let recep_info = consumer
        .receptacles()
        .iter()
        .find(|info| info.name == receptacle_name)
        .ok_or_else(|| RegistryError::BindingFailed {
            detail: format!("consumer does not have receptacle '{}'", receptacle_name),
        })?;

    // 3. Verify type compatibility
    if iface_info.type_id != recep_info.type_id {
        return Err(RegistryError::BindingFailed {
            detail: format!(
                "type mismatch: interface '{}' (TypeId {:?}) is not compatible with receptacle '{}' expecting '{}' (TypeId {:?})",
                interface_name, iface_info.type_id,
                receptacle_name, recep_info.interface_name, recep_info.type_id,
            ),
        });
    }

    // 4. Delegate to connect_receptacle_raw. The generated implementation
    //    knows the concrete interface types and queries the provider itself.
    consumer.connect_receptacle_raw(receptacle_name, provider)
}
