//! Introspection example: enumerate a component's interfaces and receptacles.

use component_framework::iunknown::IUnknown;
use component_framework::{define_component, define_interface};

define_interface! {
    pub IReader {
        fn read(&self) -> Vec<u8>;
    }
}

define_interface! {
    pub IWriter {
        fn write(&self, data: &[u8]) -> bool;
    }
}

define_interface! {
    pub ILogger {
        fn log(&self, msg: &str) -> String;
    }
}

define_component! {
    pub IOComponent {
        version: "3.1.0",
        provides: [IReader, IWriter],
        receptacles: {
            logger: ILogger,
        },
    }
}

impl IReader for IOComponent {
    fn read(&self) -> Vec<u8> {
        vec![0xDE, 0xAD]
    }
}

impl IWriter for IOComponent {
    fn write(&self, _data: &[u8]) -> bool {
        true
    }
}

fn main() {
    let comp = IOComponent::new();

    // Version
    println!("Version: {}", comp.version());

    // Enumerate provided interfaces
    println!("\nProvided interfaces:");
    for info in comp.provided_interfaces() {
        println!("  - {}", info.name);
    }

    // Enumerate receptacles
    println!("\nReceptacles:");
    for info in comp.receptacles() {
        println!("  - {} (requires {})", info.name, info.interface_name);
    }

    // Show receptacle connection state
    println!(
        "\nLogger receptacle connected: {}",
        comp.logger.is_connected()
    );
}
