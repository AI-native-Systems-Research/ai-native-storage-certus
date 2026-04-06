//! First-party vs third-party binding example.
//!
//! First-party binding: the assembler knows the concrete component types
//! and wires them directly using typed `query_interface!` and `receptacle.connect()`.
//!
//! Third-party binding: the assembler only sees `&dyn IUnknown` and wires
//! components by string names using the `bind()` free function.
//!
//! Uses `query_interface!`, `register_simple`, and the prelude for concise code.

use component_framework::binding::bind;
use component_framework::component_ref::ComponentRef;
use component_framework::iunknown::{query, IUnknown};
use component_framework::query_interface;
use component_framework::registry::ComponentRegistry;
use component_framework::{define_component, define_interface};
use std::sync::Arc;

// ---------- Interface definitions ----------

define_interface! {
    pub ILogger {
        fn log(&self, msg: &str);
    }
}

define_interface! {
    pub IStorage {
        fn get(&self, key: &str) -> Option<String>;
    }
}

// ---------- Provider components ----------

define_component! {
    pub ConsoleLogger {
        version: "1.0.0",
        provides: [ILogger],
    }
}

impl ILogger for ConsoleLogger {
    fn log(&self, msg: &str) {
        println!("  [LOG] {msg}");
    }
}

define_component! {
    pub InMemoryStorage {
        version: "1.0.0",
        provides: [IStorage],
    }
}

impl IStorage for InMemoryStorage {
    fn get(&self, key: &str) -> Option<String> {
        match key {
            "greeting" => Some("hello world".to_string()),
            "answer" => Some("42".to_string()),
            _ => None,
        }
    }
}

// ---------- Consumer component ----------

define_component! {
    pub AppService {
        version: "1.0.0",
        provides: [],
        receptacles: {
            logger: ILogger,
            storage: IStorage,
        },
    }
}

impl AppService {
    fn run(&self) {
        let logger = self.logger.get().expect("logger not connected");
        let storage = self.storage.get().expect("storage not connected");

        logger.log("AppService starting up");

        match storage.get("greeting") {
            Some(val) => logger.log(&format!("greeting = {val}")),
            None => logger.log("greeting not found"),
        }

        match storage.get("answer") {
            Some(val) => logger.log(&format!("answer = {val}")),
            None => logger.log("answer not found"),
        }

        logger.log("AppService done");
    }
}

// ---------- First-party binding ----------
// The assembler knows the concrete types and wires them directly.

fn first_party_example() {
    println!("=== First-Party Binding ===");
    println!("(Assembler knows concrete component types)\n");

    let logger = ConsoleLogger::new();
    let storage = InMemoryStorage::new();
    let app = AppService::new();

    // query_interface! eliminates the verbose dyn + Send + Sync annotation
    let ilogger: Arc<dyn ILogger + Send + Sync> = query_interface!(logger, ILogger).unwrap();
    let istorage: Arc<dyn IStorage + Send + Sync> = query_interface!(storage, IStorage).unwrap();

    app.logger.connect(ilogger).unwrap();
    app.storage.connect(istorage).unwrap();

    app.run();
    println!();
}

// ---------- Third-party binding ----------
// The assembler only sees &dyn IUnknown and uses string names.

fn third_party_example() {
    println!("=== Third-Party Binding ===");
    println!("(Assembler uses only string names and &dyn IUnknown)\n");

    let registry = ComponentRegistry::new();

    // register_simple — no type annotations needed
    registry
        .register_simple(
            "console-logger",
            || ComponentRef::from(ConsoleLogger::new()),
        )
        .unwrap();

    registry
        .register_simple("memory-storage", || {
            ComponentRef::from(InMemoryStorage::new())
        })
        .unwrap();

    registry
        .register_simple("app-service", || ComponentRef::from(AppService::new()))
        .unwrap();

    // Create components by name
    let logger = registry.create("console-logger", None).unwrap();
    let storage = registry.create("memory-storage", None).unwrap();
    let app = registry.create("app-service", None).unwrap();

    // Wire by string names
    bind(&*logger, "ILogger", &*app, "logger").unwrap();
    bind(&*storage, "IStorage", &*app, "storage").unwrap();

    // Verify wiring via introspection
    let app_arc: Arc<dyn IUnknown> = query::<dyn IUnknown>(&*app).unwrap();
    println!(
        "  App interfaces: {:?}",
        app_arc
            .provided_interfaces()
            .iter()
            .map(|i| i.name)
            .collect::<Vec<_>>()
    );
    println!(
        "  App receptacles: {:?}",
        app_arc
            .receptacles()
            .iter()
            .map(|r| format!("{} ({})", r.name, r.interface_name))
            .collect::<Vec<_>>()
    );
    println!();
}

fn main() {
    first_party_example();
    third_party_example();
}
