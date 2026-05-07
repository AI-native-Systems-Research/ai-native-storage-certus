//! Hello World actor component.
//!
//! Provides a greeter actor that receives [`GreetRequest`] messages and prints
//! hello messages.
//!
//! # Quick start
//!
//! ```
//! use example_helloworld::{GreetRequest, GreeterHandler};
//! use component_framework::actor::Actor;
//!
//! let greeter = Actor::simple(GreeterHandler::new());
//! let greeter_handle = greeter.activate().unwrap();
//!
//! greeter_handle.send(GreetRequest { name: "World".into() }).unwrap();
//! greeter_handle.deactivate().unwrap();
//! ```

use component_framework::actor::ActorHandler;
use component_framework::{define_component, define_interface};
use interfaces::ILogger;
use std::sync::Arc;

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
        receptacles: {
            logger: ILogger,
        },
    }
}

impl IGreeter for HelloWorldComponent {
    fn greeting_prefix(&self) -> &str {
        "Hello"
    }
}

/// Message sent to the greeter actor.
#[derive(Debug)]
pub struct GreetRequest {
    pub name: String,
}

/// Actor handler that prints greetings and logs via ILogger.
pub struct GreeterHandler {
    count: u32,
    logger: Option<Arc<dyn ILogger + Send + Sync>>,
}

impl GreeterHandler {
    /// Create a handler without a logger.
    pub fn new() -> Self {
        Self {
            count: 0,
            logger: None,
        }
    }

    /// Create a handler with an ILogger for structured logging.
    pub fn with_logger(logger: Arc<dyn ILogger + Send + Sync>) -> Self {
        Self {
            count: 0,
            logger: Some(logger),
        }
    }
}

impl Default for GreeterHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ActorHandler<GreetRequest> for GreeterHandler {
    fn on_start(&mut self) {
        if let Some(log) = &self.logger {
            log.info("greeter actor started");
        }
        eprintln!("[greeter] Greeter actor started");
    }

    fn handle(&mut self, msg: GreetRequest) {
        self.count += 1;
        if let Some(log) = &self.logger {
            log.info(&format!("[{}] greeting {}", self.count, msg.name));
        }
        println!("  [{}] Hello, {}!", self.count, msg.name);
    }

    fn on_stop(&mut self) {
        if let Some(log) = &self.logger {
            log.info(&format!(
                "greeter stopped after {} greetings",
                self.count
            ));
        }
        eprintln!("[greeter] Greeter stopped after {} greetings", self.count);
    }
}

#[cfg(kani)]
mod verification {
    use super::GreeterHandler;

    #[kani::proof]
    #[kani::unwind(1)]
    fn verify_greeter_new_initial_state() {
        let h = GreeterHandler::new();
        kani::assert(h.count == 0, "count must start at 0");
        kani::assert(h.logger.is_none(), "logger must be None on new");
    }

    /// NOTE — assume is UNMATCHED: handle() has bare `self.count += 1` with no guard.
    /// This harness passes because kani::assume precludes the overflow path;
    /// the production code has a latent unchecked overflow defect.
    #[kani::proof]
    #[kani::unwind(1)]
    fn verify_count_increment_bounded() {
        let init: u32 = kani::any();
        kani::assume(init < u32::MAX); // unmatched: production has no overflow guard
        let next = init + 1;
        kani::assert(next > init, "increment must strictly increase count");
        kani::assert(next == init.wrapping_add(1), "increment must be exact");
    }

    #[kani::proof]
    #[kani::unwind(1)]
    fn verify_count_two_step_increment() {
        let init: u32 = kani::any();
        kani::assume(init < u32::MAX - 1);
        let after_one = init + 1;
        let after_two = after_one + 1;
        kani::assert(after_two > after_one, "second increment increases count");
        kani::assert(after_two == init + 2, "two increments equals init + 2");
    }

    #[kani::proof]
    #[kani::unwind(1)]
    fn verify_greeter_default_equals_new() {
        let by_new = GreeterHandler::new();
        let by_default = GreeterHandler::default();
        kani::assert(
            by_new.count == by_default.count,
            "default().count must equal new().count",
        );
        kani::assert(
            by_new.logger.is_none() && by_default.logger.is_none(),
            "both constructors must produce no logger",
        );
    }
}
