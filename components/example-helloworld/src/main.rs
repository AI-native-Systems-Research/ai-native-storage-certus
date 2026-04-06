//! Actor-based Hello World example.
//!
//! Demonstrates defining a component with an actor that receives greeting
//! requests and prints hello messages. Uses the component framework's
//! `Actor`, `ActorHandler`, and `define_component!`/`define_interface!` macros.

use component_framework::actor::{Actor, ActorHandler};
use component_framework::iunknown::IUnknown;
use component_framework::{define_component, define_interface};

// Define an interface for the greeter component.
define_interface! {
    pub IGreeter {
        fn greeting_prefix(&self) -> &str;
    }
}

// Define the component.
define_component! {
    pub HelloWorldComponent {
        version: "0.1.0",
        provides: [IGreeter],
    }
}

impl IGreeter for HelloWorldComponent {
    fn greeting_prefix(&self) -> &str {
        "Hello"
    }
}

/// Message sent to the greeter actor.
#[derive(Debug)]
struct GreetRequest {
    name: String,
}

/// Actor handler that prints greetings.
struct GreeterHandler {
    count: u32,
}

impl ActorHandler<GreetRequest> for GreeterHandler {
    fn on_start(&mut self) {
        println!("Greeter actor started");
    }

    fn handle(&mut self, msg: GreetRequest) {
        self.count += 1;
        println!("  [{}] Hello, {}!", self.count, msg.name);
    }

    fn on_stop(&mut self) {
        println!("Greeter actor stopped after {} greetings", self.count);
    }
}

fn main() {
    println!("=== Actor Hello World Example ===\n");

    // Instantiate the component and show its version.
    let comp = HelloWorldComponent::new();
    println!("Component version: {}", comp.version());

    // Create and activate the greeter actor.
    let actor = Actor::simple(GreeterHandler { count: 0 });
    let handle = actor.activate().unwrap();

    // Send some greeting requests.
    let names = ["World", "Rust", "Certus", "Actors"];
    for name in names {
        handle
            .send(GreetRequest {
                name: name.to_string(),
            })
            .unwrap();
    }

    // Deactivate the actor (processes remaining messages, then calls on_stop).
    handle.deactivate().unwrap();

    println!("\n=== Done ===");
}
