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

use creusot_std::prelude::*;
#[allow(unused_imports)]
use std::vec;

#[cfg(not(creusot))]
use component_framework::actor::ActorHandler;
use component_framework::{define_component, define_interface};
use interfaces::ILogger;
#[cfg(not(creusot))]
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

#[cfg(creusot)]
impl HelloWorldComponent {
    #[trusted]
    pub fn new() -> ::std::sync::Arc<Self> {
        panic!()
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
    #[cfg_attr(creusot, allow(dead_code))]
    count: u32,
    #[cfg(not(creusot))]
    logger: Option<Arc<dyn ILogger + Send + Sync>>,
}

impl GreeterHandler {
    /// Create a handler without a logger.
    pub fn new() -> Self {
        Self {
            count: 0,
            #[cfg(not(creusot))]
            logger: None,
        }
    }

    /// Create a handler with an ILogger for structured logging.
    #[cfg(not(creusot))]
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

#[cfg(not(creusot))]
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
