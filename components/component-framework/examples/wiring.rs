//! Wiring example: connect two components via a receptacle.
//!
//! A StorageComponent provides IStorage.
//! A CacheComponent requires IStorage (via receptacle) and provides ICache.
//!
//! Uses `query_interface!` macro for concise interface queries.

use component_framework::query_interface;
use component_framework::{define_component, define_interface};
use std::sync::Arc;

// --- Interfaces ---

define_interface! {
    pub IStorage {
        fn read(&self, key: &str) -> Option<String>;
        fn write(&self, key: &str, value: &str) -> bool;
    }
}

define_interface! {
    pub ICache {
        fn get(&self, key: &str) -> Option<String>;
    }
}

// --- Storage component (provides IStorage) ---

define_component! {
    pub StorageComponent {
        version: "1.0.0",
        provides: [IStorage],
    }
}

impl IStorage for StorageComponent {
    fn read(&self, key: &str) -> Option<String> {
        if key == "config" {
            Some("value=42".to_string())
        } else {
            None
        }
    }

    fn write(&self, _key: &str, _value: &str) -> bool {
        true
    }
}

// --- Cache component (requires IStorage, provides ICache) ---

define_component! {
    pub CacheComponent {
        version: "2.0.0",
        provides: [ICache],
        receptacles: {
            storage: IStorage,
        },
    }
}

impl ICache for CacheComponent {
    fn get(&self, key: &str) -> Option<String> {
        let storage = self.storage.get().ok()?;
        storage.read(key)
    }
}

fn main() {
    let storage = StorageComponent::new();
    let cache = CacheComponent::new();

    // Query IStorage and wire via receptacle
    let istorage: Arc<dyn IStorage + Send + Sync> = query_interface!(storage, IStorage).unwrap();
    cache.storage.connect(istorage).unwrap();
    println!("Receptacle connected: {}", cache.storage.is_connected());

    // Use the cache
    let icache: Arc<dyn ICache + Send + Sync> = query_interface!(cache, ICache).unwrap();

    match icache.get("config") {
        Some(val) => println!("Cache hit: {val}"),
        None => println!("Cache miss"),
    }

    match icache.get("missing") {
        Some(val) => println!("Cache hit: {val}"),
        None => println!("Cache miss"),
    }

    // Disconnect and reconnect
    cache.storage.disconnect().unwrap();
    println!(
        "After disconnect: connected={}",
        cache.storage.is_connected()
    );

    let istorage2: Arc<dyn IStorage + Send + Sync> = query_interface!(storage, IStorage).unwrap();
    cache.storage.connect(istorage2).unwrap();
    println!("Re-connected: {}", cache.storage.is_connected());
}
