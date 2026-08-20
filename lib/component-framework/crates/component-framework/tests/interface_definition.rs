use component_macros::define_interface;

// T017: define_interface! generates valid trait with methods
define_interface! {
    pub IStorage {
        fn read(&self, key: &str) -> Option<Vec<u8>>;
        fn write(&self, key: &str, value: &[u8]) -> Result<(), String>;
        fn delete(&self, key: &str) -> bool;
    }
}

// T019: define_interface! with lifetime parameters compiles
define_interface! {
    pub IBorrower {
        fn borrow_data<'a>(&'a self) -> &'a [u8];
        fn borrow_str<'a>(&'a self, key: &'a str) -> &'a str;
    }
}

// Multi-method interface
define_interface! {
    pub ILogger {
        fn log(&self, level: u8, message: &str);
        fn flush(&self);
    }
}

struct MockStorage {
    data: std::collections::HashMap<String, Vec<u8>>,
}

impl IStorage for MockStorage {
    fn read(&self, key: &str) -> Option<Vec<u8>> {
        self.data.get(key).cloned()
    }
    fn write(&self, key: &str, value: &[u8]) -> Result<(), String> {
        // Interior mutability not needed for test
        let _ = (key, value);
        Ok(())
    }
    fn delete(&self, key: &str) -> bool {
        let _ = key;
        false
    }
}

struct MockBorrower {
    data: Vec<u8>,
}

impl IBorrower for MockBorrower {
    fn borrow_data(&self) -> &[u8] {
        &self.data
    }
    fn borrow_str<'a>(&'a self, _key: &'a str) -> &'a str {
        "borrowed"
    }
}

#[test]
fn interface_generates_valid_trait() {
    let storage = MockStorage {
        data: std::collections::HashMap::new(),
    };
    assert!(storage.read("missing").is_none());
}

#[test]
fn interface_usable_as_trait_object() {
    let storage = MockStorage {
        data: std::collections::HashMap::new(),
    };
    let dyn_ref: &dyn IStorage = &storage;
    assert!(dyn_ref.read("key").is_none());
}

#[test]
fn interface_with_lifetimes_compiles() {
    let borrower = MockBorrower {
        data: vec![1, 2, 3],
    };
    assert_eq!(borrower.borrow_data(), &[1, 2, 3]);
    assert_eq!(borrower.borrow_str("x"), "borrowed");
}

#[test]
fn interface_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<MockStorage>();
    assert_send_sync::<MockBorrower>();
}

#[test]
fn trait_object_is_send_sync() {
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    assert_send_sync::<dyn IStorage + Send + Sync>();
    assert_send_sync::<dyn IBorrower + Send + Sync>();
}

// T018: Interface usable as type bound in separate compilation unit
// (this file IS a separate compilation unit from the macro crate)
fn generic_function<S: IStorage>(storage: &S) -> Option<Vec<u8>> {
    storage.read("test")
}

#[test]
fn interface_as_generic_bound() {
    let storage = MockStorage {
        data: std::collections::HashMap::new(),
    };
    assert!(generic_function(&storage).is_none());
}
