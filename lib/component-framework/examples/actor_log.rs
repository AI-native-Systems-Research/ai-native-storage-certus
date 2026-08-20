//! Logging actor example using the framework's built-in [`LogHandler`].
//!
//! Demonstrates:
//! - Stderr-only logging with all severity levels
//! - Dual output (stderr + file)
//! - Minimum level filtering
//!
//! # Running
//!
//! ```sh
//! cargo run --example actor_log
//! ```

use component_core::actor::Actor;
use component_core::log::{LogHandler, LogLevel, LogMessage};
use component_framework::{define_component, define_interface};

define_interface! {
    pub IExampleLog {
        fn beep(&self) -> String;
    }
}

define_component! {
    pub ExampleLog {
        version: "0.1.0",
        provides: [IExampleLog],
    }
}

impl IExampleLog for ExampleLog {
    fn beep(&self) -> String {
        "beep".to_string()
    }
}

fn main() {
    println!("=== Logging Actor Example ===\n");

    // --- 1. Stderr-only logging ---
    println!("--- stderr-only (all levels) ---");
    let actor = Actor::simple(LogHandler::new());
    let handle = actor.activate().unwrap();

    handle
        .send(LogMessage::debug("verbose trace data"))
        .unwrap();
    handle.send(LogMessage::info("server started")).unwrap();
    handle
        .send(LogMessage::warn("high latency detected"))
        .unwrap();
    handle.send(LogMessage::error("connection lost")).unwrap();

    handle.deactivate().unwrap();
    println!();

    // --- 2. Stderr + file logging ---
    let log_path = "/tmp/actor_log_example.log";
    println!("--- stderr + file ({log_path}) ---");

    let handler = LogHandler::with_file(log_path).expect("failed to open log file");
    let actor = Actor::simple(handler);
    let handle = actor.activate().unwrap();

    handle
        .send(LogMessage::info("writing to console and file"))
        .unwrap();
    handle
        .send(LogMessage::warn("this goes to both outputs"))
        .unwrap();

    handle.deactivate().unwrap();

    // Read and display the log file contents.
    println!("\nFile contents of {log_path}:");
    let contents = std::fs::read_to_string(log_path).unwrap();
    for line in contents.lines() {
        println!("  {line}");
    }
    println!();

    // --- 3. Minimum level filtering ---
    println!("--- filtered (min_level = Warn) ---");

    let handler = LogHandler::new().with_min_level(LogLevel::Warn);
    let actor = Actor::simple(handler);
    let handle = actor.activate().unwrap();

    handle
        .send(LogMessage::debug("filtered out (debug)"))
        .unwrap();
    handle
        .send(LogMessage::info("filtered out (info)"))
        .unwrap();
    handle
        .send(LogMessage::warn("this passes the filter"))
        .unwrap();
    handle.send(LogMessage::error("this also passes")).unwrap();

    handle.deactivate().unwrap();

    // Clean up
    let _ = std::fs::remove_file(log_path);

    println!("\nDone.");
}
