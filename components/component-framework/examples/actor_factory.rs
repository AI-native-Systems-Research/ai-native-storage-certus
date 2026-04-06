//! Actor factory example: create actor components through the ComponentRegistry.
//!
//! Demonstrates:
//! - Registering an actor factory with typed configuration
//! - Creating actor components via the registry
//! - Querying `ISender<M>` through `IUnknown` to send messages
//! - Lifecycle management (activate / deactivate)
//! - Using the framework's built-in [`LogHandler`] and [`LogMessage`]
//! - Using `Actor::simple()` and `query_interface!` for concise code
//!
//! # Running
//!
//! ```sh
//! cargo run --example actor_factory
//! ```

use component_core::actor::Actor;
use component_core::channel::ISender;
use component_core::component_ref::ComponentRef;
use component_core::error::RegistryError;
use component_core::log::{LogHandler, LogMessage};
use component_core::query_interface;
use component_core::registry::ComponentRegistry;
use component_framework::{define_component, define_interface};
use std::any::Any;
use std::sync::Arc;

define_interface! {
    pub IExampleFactory {
        fn val(&self) -> u32;
    }
}

define_component! {
    pub ExampleFactory {
        version: "0.1.0",
        provides: [IExampleFactory],
    }
}

impl IExampleFactory for ExampleFactory {
    fn val(&self) -> u32 {
        9
    }
}

// ---------------------------------------------------------------------------
// 1. Define factory configuration
// ---------------------------------------------------------------------------

/// Configuration passed to the actor factory.
struct LogActorConfig {
    log_path: Option<String>,
    capacity: usize,
}

// ---------------------------------------------------------------------------
// 2. Register and use the factory
// ---------------------------------------------------------------------------

fn main() {
    println!("=== Actor Factory Example ===\n");

    // --- Registry setup ---
    let registry = ComponentRegistry::new();

    // Register a factory for "logger" actors.
    registry
        .register(
            "logger",
            |config: Option<&dyn Any>| -> Result<ComponentRef, RegistryError> {
                let cfg = config
                    .and_then(|c| c.downcast_ref::<LogActorConfig>())
                    .ok_or_else(|| RegistryError::FactoryFailed {
                        name: "logger".into(),
                        source: "LogActorConfig required".into(),
                    })?;

                let handler = match &cfg.log_path {
                    Some(path) => {
                        LogHandler::with_file(path).map_err(|e| RegistryError::FactoryFailed {
                            name: "logger".into(),
                            source: format!("failed to open log file: {e}"),
                        })?
                    }
                    None => LogHandler::new(),
                };

                let actor = Actor::with_capacity(handler, cfg.capacity, |panic_payload| {
                    eprintln!("Logger actor panicked: {panic_payload:?}");
                });

                Ok(ComponentRef::from(Arc::new(actor)))
            },
        )
        .unwrap();

    println!("Registered factories: {:?}", registry.list());

    // --- Create actor via factory ---
    let config = LogActorConfig {
        log_path: None, // stderr only
        capacity: 256,
    };

    let comp: ComponentRef = registry.create("logger", Some(&config)).unwrap();
    println!("Created component: version={}", comp.version());
    println!(
        "Interfaces: {:?}",
        comp.provided_interfaces()
            .iter()
            .map(|i| i.name)
            .collect::<Vec<_>>()
    );

    // --- Query ISender through IUnknown ---
    let sender: Arc<dyn ISender<LogMessage> + Send + Sync> =
        query_interface!(comp, ISender<LogMessage>).unwrap();

    // --- Lifecycle management ---
    let actor2 = Actor::simple(LogHandler::new());
    let handle = actor2.activate().unwrap();
    println!("\nActor activated — sending messages via ActorHandle...");

    handle.send(LogMessage::info("direct message 1")).unwrap();
    handle.send(LogMessage::warn("direct warning")).unwrap();
    handle.deactivate().unwrap();

    // --- ISender messaging pattern ---
    println!("\nCreating a third actor and sending via ISender...");

    let actor3 = Actor::simple(LogHandler::new());

    // Query ISender before activation
    let sender3: Arc<dyn ISender<LogMessage> + Send + Sync> =
        query_interface!(actor3, ISender<LogMessage>).unwrap();

    let handle3 = actor3.activate().unwrap();

    sender3
        .send(LogMessage::info("via ISender: hello"))
        .unwrap();
    sender3
        .send(LogMessage::warn("via ISender: caution"))
        .unwrap();
    sender3.send(LogMessage::info("via ISender: done")).unwrap();

    handle3.deactivate().unwrap();

    // Show that the factory-created component's ISender is also functional
    println!("\nFactory-created component ISender is available: {}", {
        match sender.try_send(LogMessage::info("test")) {
            Ok(()) => "sent (queued)",
            Err(e) => {
                if format!("{e}").contains("full") {
                    "channel full (actor not activated)"
                } else {
                    "error"
                }
            }
        }
    });

    println!("\nDone.");
}
