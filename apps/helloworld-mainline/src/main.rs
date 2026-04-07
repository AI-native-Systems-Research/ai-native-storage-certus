//! Hello World mainline application.
//!
//! Instantiates the logger and helloworld components from the components
//! directory, wires the greeter actor to the logger actor, and sends a
//! sequence of greeting requests.

use component_framework::actor::Actor;
use component_framework::iunknown::{query, IUnknown};
use example_helloworld::{GreetRequest, GreeterHandler, HelloWorldComponent, IGreeter};
use example_logger::{ConsoleLogHandler, ConsoleLogRequest, ILogger, LogLevel, LoggerComponent};

fn main() {
    println!("=== Hello World Mainline Application ===\n");

    // --- Instantiate components ---
    let logger_comp = LoggerComponent::new();
    let greeter_comp = HelloWorldComponent::new();

    println!(
        "Logger  component: version={}, name={}",
        logger_comp.version(),
        query::<dyn ILogger + Send + Sync>(&*logger_comp)
            .expect("ILogger not found")
            .name(),
    );
    println!(
        "Greeter component: version={}, prefix=\"{}\"",
        greeter_comp.version(),
        query::<dyn IGreeter + Send + Sync>(&*greeter_comp)
            .expect("IGreeter not found")
            .greeting_prefix(),
    );
    println!();

    // --- Start the logger actor ---
    let logger_actor = Actor::simple(ConsoleLogHandler::new());
    let logger_handle = logger_actor.activate().unwrap();

    // Application-level log via the logger actor.
    logger_handle
        .send(ConsoleLogRequest {
            level: LogLevel::Info,
            source: "app".into(),
            text: "Application started".into(),
        })
        .unwrap();

    // --- Start the greeter actor, wired to the logger ---
    let greeter_actor = Actor::simple(GreeterHandler::new(logger_handle));
    let greeter_handle = greeter_actor.activate().unwrap();

    // Send greeting requests.
    for name in ["World", "Rust", "Certus", "Actors"] {
        greeter_handle
            .send(GreetRequest {
                name: name.to_string(),
            })
            .unwrap();
    }

    // Shutdown: deactivating the greeter joins its thread, which sends a
    // final log message and then drops the logger handle, causing the
    // logger actor to drain and stop.
    greeter_handle.deactivate().unwrap();

    println!("\n=== Done ===");
}
