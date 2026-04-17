//! Hello World actor component.
//!
//! Provides a greeter actor that receives [`GreetRequest`] messages, prints
//! hello messages, and forwards log entries to a logger actor from
//! [`console_logger`].
//!
//! # Quick start
//!
//! ```
//! use example_helloworld::{GreetRequest, GreeterHandler};
//! use console_logger::{ConsoleLogHandler, ConsoleLogRequest};
//! use component_framework::actor::Actor;
//!
//! let logger = Actor::simple(ConsoleLogHandler::new());
//! let logger_handle = logger.activate().unwrap();
//!
//! let greeter = Actor::simple(GreeterHandler::new(logger_handle));
//! let greeter_handle = greeter.activate().unwrap();
//!
//! greeter_handle.send(GreetRequest { name: "World".into() }).unwrap();
//! greeter_handle.deactivate().unwrap();
//! ```

use component_framework::actor::{ActorHandle, ActorHandler};
use component_framework::{define_component, define_interface};
use console_logger::{ConsoleLogRequest, LogLevel};

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
pub struct GreetRequest {
    pub name: String,
}

/// Actor handler that prints greetings and logs to a logger actor.
pub struct GreeterHandler {
    count: u32,
    logger: ActorHandle<ConsoleLogRequest>,
}

impl GreeterHandler {
    pub fn new(logger: ActorHandle<ConsoleLogRequest>) -> Self {
        Self { count: 0, logger }
    }
}

impl ActorHandler<GreetRequest> for GreeterHandler {
    fn on_start(&mut self) {
        self.logger
            .send(ConsoleLogRequest {
                level: LogLevel::Info,
                source: "greeter".into(),
                text: "Greeter actor started".into(),
            })
            .unwrap();
    }

    fn handle(&mut self, msg: GreetRequest) {
        self.count += 1;
        println!("  [{}] Hello, {}!", self.count, msg.name);

        self.logger
            .send(ConsoleLogRequest {
                level: LogLevel::Debug,
                source: "greeter".into(),
                text: format!("Processed greeting #{} for {}", self.count, msg.name),
            })
            .unwrap();
    }

    fn on_stop(&mut self) {
        self.logger
            .send(ConsoleLogRequest {
                level: LogLevel::Info,
                source: "greeter".into(),
                text: format!("Greeter stopped after {} greetings", self.count),
            })
            .unwrap();
    }
}
