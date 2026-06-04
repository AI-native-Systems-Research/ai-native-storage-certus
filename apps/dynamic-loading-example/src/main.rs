//! Dynamic component loading example (Rust-ABI).
//!
//! Demonstrates loading a component from a shared library (`.so`) at runtime
//! using `libloading`, then querying its interfaces via `IUnknown` — exactly
//! the same way statically-linked components are used.
//!
//! This works because the host binary and the plugin dylib both dynamically
//! link the same `component-core` and `example-helloworld` shared libraries,
//! ensuring a single set of `TypeId` values across the process.
//!
//! **Requirement**: everything must be built with the same `rustc` version.

use component_core::component_ref::ComponentRef;
use component_core::iunknown::query;
use example_helloworld::IGreeter;
use libloading::{Library, Symbol};
use std::env;
use std::path::PathBuf;
use std::sync::Arc;

fn find_dylib() -> PathBuf {
    let exe = env::current_exe().expect("cannot determine executable path");
    let dir = exe.parent().expect("executable has no parent directory");

    let candidates = [
        dir.join("libexample_helloworld_dylib.so"),
        dir.join("deps/libexample_helloworld_dylib.so"),
    ];

    for path in &candidates {
        if path.exists() {
            return path.clone();
        }
    }

    panic!(
        "Could not find libexample_helloworld_dylib.so — \
         build it first with: cargo build -p example-helloworld-dylib\n\
         Searched: {:?}",
        candidates
    );
}

fn main() {
    println!("=== Dynamic Component Loading Example (Rust-ABI) ===\n");

    // 1. Locate and load the shared library at runtime.
    let lib_path = find_dylib();
    println!("Loading component from: {}\n", lib_path.display());

    // SAFETY: We trust the .so was built with the same compiler and links
    // against the same shared crates (component-core, example-helloworld).
    let lib = unsafe { Library::new(&lib_path) }.expect("failed to load shared library");

    // 2. Look up the Rust-ABI factory function.
    //    No C shim needed — the symbol returns a ComponentRef directly.
    let create: Symbol<fn() -> ComponentRef> =
        unsafe { lib.get(b"create_component") }.expect("symbol 'create_component' not found");

    // 3. Create the component via the factory.
    let comp = create();

    println!("Component loaded successfully!");
    println!("  Version: {}", comp.version());
    println!(
        "  Provided interfaces: {:?}",
        comp.provided_interfaces()
            .iter()
            .map(|i| i.name)
            .collect::<Vec<_>>()
    );
    println!();

    // 4. Query IGreeter via IUnknown — direct trait dispatch, no C-ABI wrapper.
    let greeter: Arc<dyn IGreeter + Send + Sync> =
        query::<dyn IGreeter + Send + Sync>(&*comp).expect("component does not provide IGreeter");

    println!("Queried IGreeter interface:");
    println!("  greeting_prefix() = \"{}\"", greeter.greeting_prefix());
    println!();

    // 5. Clean up — dropping the ComponentRef decrements the Arc.
    drop(greeter);
    drop(comp);

    println!("Component dropped (Arc ref-count reached zero).");
    println!("\n=== Done ===");
}
