# Quickstart: Registry, Reference Counting, and Binding

## 1. Registry — Register and Create Components

```rust
use component_framework::{define_interface, define_component};
use component_framework::registry::{ComponentRegistry, ComponentRef};
use component_framework::iunknown::{IUnknown, query};
use std::sync::Arc;

// Define an interface and component
define_interface! {
    pub IGreeter {
        fn greet(&self, name: &str) -> String;
    }
}

define_component! {
    pub HelloComponent {
        version: "1.0.0",
        provides: [IGreeter],
    }
}

impl IGreeter for HelloComponent {
    fn greet(&self, name: &str) -> String {
        format!("Hello, {name}!")
    }
}

// Create a registry and register the component
let registry = ComponentRegistry::new();
registry.register("hello", |_config| {
    Ok(ComponentRef::from(HelloComponent::new()))
}).unwrap();

// Create a component by name
let comp = registry.create("hello", None).unwrap();
let greeter: Arc<dyn IGreeter + Send + Sync> =
    query::<dyn IGreeter + Send + Sync>(&*comp).unwrap();
assert_eq!(greeter.greet("world"), "Hello, world!");
```

## 2. Reference Counting — Attach and Release

```rust
// comp has reference count = 1
let comp2 = comp.attach();  // count = 2
let comp3 = comp.attach();  // count = 3

drop(comp2);                // count = 2 (release)
drop(comp3);                // count = 1
drop(comp);                 // count = 0 → component destroyed
```

## 3. First-Party Binding (existing, unchanged)

```rust
// Direct wiring — the integrator knows both concrete types
let logger_comp = registry.create("logger", None).unwrap();
let app_comp = registry.create("app", None).unwrap();

let ilogger: Arc<dyn ILogger + Send + Sync> =
    query::<dyn ILogger + Send + Sync>(&*logger_comp).unwrap();
app_comp_concrete.logger.connect(ilogger).unwrap();
```

## 4. Third-Party Binding (new)

```rust
use component_framework::binding::bind;

// The assembler doesn't know concrete types — uses string names only
let logger_comp = registry.create("logger", None).unwrap();
let app_comp = registry.create("app", None).unwrap();

// Wire by interface name and receptacle name
bind(&*logger_comp, "ILogger", &*app_comp, "logger").unwrap();

// The app component can now use its logger receptacle
```

## 5. End-to-End: Registry-Driven Assembly

```rust
let registry = ComponentRegistry::new();

// Register factories
registry.register("logger", |_| Ok(ComponentRef::from(LoggerComponent::new()))).unwrap();
registry.register("app", |_| Ok(ComponentRef::from(AppComponent::new()))).unwrap();

// Create by name
let logger = registry.create("logger", None).unwrap();
let app = registry.create("app", None).unwrap();

// Wire using third-party binding
bind(&*logger, "ILogger", &*app, "logger").unwrap();

// Use the assembled system
let service: Arc<dyn IAppService + Send + Sync> =
    query::<dyn IAppService + Send + Sync>(&*app).unwrap();
service.do_work();  // internally uses the wired logger
```
