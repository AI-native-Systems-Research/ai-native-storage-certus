use std::any::{Any, TypeId};
use std::collections::HashMap;

use crate::interface::InterfaceInfo;

/// Internal storage for a component's provided interfaces.
///
/// Maps `TypeId` to type-erased `Arc<dyn IFoo + Send + Sync>` values
/// (boxed as `dyn Any`). Populated once at component construction and
/// immutable thereafter.
///
/// # Examples
///
/// ```
/// use component_core::component::InterfaceMap;
/// use std::any::TypeId;
/// use std::sync::Arc;
///
/// let mut map = InterfaceMap::new();
///
/// // Insert a boxed Arc<dyn Any> keyed by TypeId
/// let key = TypeId::of::<String>();
/// let value: Arc<String> = Arc::new("hello".to_string());
/// map.insert(key, "IExample", Box::new(value));
///
/// assert!(map.lookup(key).is_some());
/// assert!(map.lookup(TypeId::of::<u32>()).is_none());
/// assert_eq!(map.info().len(), 1);
/// assert_eq!(map.info()[0].name, "IExample");
/// ```
pub struct InterfaceMap {
    map: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    info: Vec<InterfaceInfo>,
}

impl InterfaceMap {
    /// Creates an empty `InterfaceMap`.
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            info: Vec::new(),
        }
    }

    /// Inserts an interface entry.
    ///
    /// The `value` should be a `Box`-ed `Arc<dyn IFoo + Send + Sync>`.
    /// The `name` is used for introspection metadata.
    pub fn insert(
        &mut self,
        type_id: TypeId,
        name: &'static str,
        value: Box<dyn Any + Send + Sync>,
    ) {
        self.map.insert(type_id, value);
        self.info.push(InterfaceInfo { type_id, name });
    }

    /// Looks up an interface by `TypeId`.
    ///
    /// Returns a reference to the type-erased value if found.
    pub fn lookup(&self, type_id: TypeId) -> Option<&(dyn Any + Send + Sync)> {
        self.map.get(&type_id).map(|b| &**b)
    }

    /// Returns metadata for all stored interfaces.
    ///
    /// Each entry contains the `TypeId` and the string name of an interface
    /// that was inserted into the map.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::component::InterfaceMap;
    /// use std::any::TypeId;
    /// use std::sync::Arc;
    ///
    /// let mut map = InterfaceMap::new();
    /// map.insert(TypeId::of::<Arc<String>>(), "IName", Box::new(Arc::new("val".to_string())));
    ///
    /// let info = map.info();
    /// assert_eq!(info.len(), 1);
    /// assert_eq!(info[0].name, "IName");
    /// ```
    pub fn info(&self) -> &[InterfaceInfo] {
        &self.info
    }
}

impl Default for InterfaceMap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn insert_and_lookup() {
        let mut map = InterfaceMap::new();
        let key = TypeId::of::<String>();
        let value: Arc<String> = Arc::new("test".to_string());
        map.insert(key, "ITest", Box::new(value.clone()));

        let result = map.lookup(key).unwrap();
        let recovered = result.downcast_ref::<Arc<String>>().unwrap();
        assert_eq!(**recovered, "test");
    }

    #[test]
    fn lookup_missing_returns_none() {
        let map = InterfaceMap::new();
        assert!(map.lookup(TypeId::of::<u64>()).is_none());
    }

    #[test]
    fn info_reflects_inserts() {
        let mut map = InterfaceMap::new();
        map.insert(TypeId::of::<u8>(), "IAlpha", Box::new(42u8));
        map.insert(TypeId::of::<u16>(), "IBeta", Box::new(99u16));
        assert_eq!(map.info().len(), 2);
        assert_eq!(map.info()[0].name, "IAlpha");
        assert_eq!(map.info()[1].name, "IBeta");
    }

    #[test]
    fn default_is_empty() {
        let map = InterfaceMap::default();
        assert_eq!(map.info().len(), 0);
    }
}
