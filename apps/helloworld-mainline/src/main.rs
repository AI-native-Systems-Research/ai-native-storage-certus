//! Hello World mainline application.
//!
//! Instantiates the helloworld component, starts a greeter actor, and sends a
//! sequence of greeting requests.

use component_framework::actor::Actor;
use component_framework::iunknown::{query, IUnknown};
use example_helloworld::{GreetRequest, GreeterHandler, HelloWorldComponent, IGreeter};

#[expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "example binary: a missing interface or a failed actor start/send/stop ends the \
              demo, so it aborts"
)]
fn main() {
    println!("=== Hello World Mainline Application ===\n");

    // --- Instantiate components ---
    let greeter_comp = HelloWorldComponent::new();

    println!(
        "Greeter component: version={}, prefix=\"{}\"",
        greeter_comp.version(),
        query::<dyn IGreeter + Send + Sync>(&*greeter_comp)
            .expect("IGreeter not found")
            .greeting_prefix(),
    );
    println!();

    // --- Start the greeter actor ---
    let greeter_actor = Actor::simple(GreeterHandler::new());
    let greeter_handle = greeter_actor.activate().unwrap();

    // Send greeting requests.
    for name in ["World", "Rust", "Certus", "Actors"] {
        greeter_handle
            .send(GreetRequest {
                name: name.to_string(),
            })
            .unwrap();
    }

    greeter_handle.deactivate().unwrap();

    println!("\n=== Done ===");
}
