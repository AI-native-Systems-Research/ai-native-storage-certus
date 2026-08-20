# Quickstart: Channel Backend Benchmarks

**Feature**: 004-channel-benchmarks
**Date**: 2026-03-31

## Using a Third-Party Channel Backend

All third-party channel backends work identically to the built-in channels. They implement `IUnknown` and provide `ISender<T>` / `IReceiver<T>` via `query()`.

### Example: Crossbeam Bounded Channel

```rust
use component_framework::channel::crossbeam_bounded::CrossbeamBoundedChannel;
use component_framework::channel::ISender;
use component_framework::iunknown::query;
use std::sync::Arc;

// Create a crossbeam-backed channel component
let ch = CrossbeamBoundedChannel::<u32>::new(1024);

// Query ISender via IUnknown (component model)
let sender: Arc<dyn ISender<u32> + Send + Sync> =
    query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();

// Or use the direct API
let rx = ch.receiver().unwrap();

sender.send(42).unwrap();
assert_eq!(rx.recv().unwrap(), 42);
```

### Example: rtrb SPSC Channel

```rust
use component_framework::channel::rtrb_spsc::RtrbChannel;
use component_framework::channel::{ISender, IReceiver};
use component_framework::iunknown::query;
use std::sync::Arc;

let ch = RtrbChannel::<u64>::new(1024);

let tx: Arc<dyn ISender<u64> + Send + Sync> =
    query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap();
let rx: Arc<dyn IReceiver<u64> + Send + Sync> =
    query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();

// Second sender query fails (SPSC)
assert!(query::<dyn ISender<u64> + Send + Sync>(&ch).is_none());

tx.send(100).unwrap();
assert_eq!(rx.recv().unwrap(), 100);
```

### Example: Drop-in Replacement

Because all backends implement the same `ISender<T>` / `IReceiver<T>` traits, you can swap channel backends without changing consumer code:

```rust
use component_framework::channel::{ISender, IReceiver};
use component_framework::iunknown::query;
use std::sync::Arc;

fn use_channel(ch: &dyn component_framework::iunknown::IUnknown) {
    let tx: Arc<dyn ISender<u32> + Send + Sync> =
        query::<dyn ISender<u32> + Send + Sync>(ch).unwrap();
    let rx: Arc<dyn IReceiver<u32> + Send + Sync> =
        query::<dyn IReceiver<u32> + Send + Sync>(ch).unwrap();

    tx.send(1).unwrap();
    assert_eq!(rx.recv().unwrap(), 1);
}

// Works with any backend:
use component_framework::channel::spsc::SpscChannel;
use_channel(&SpscChannel::<u32>::new(64));

use component_framework::channel::crossbeam_bounded::CrossbeamBoundedChannel;
use_channel(&CrossbeamBoundedChannel::<u32>::new(64));
```

## Running Benchmarks

```bash
# Run all channel benchmarks
cargo bench --bench channel_spsc_benchmark
cargo bench --bench channel_mpsc_benchmark
cargo bench --bench channel_latency_benchmark

# Run all tests (including new backend tests)
cargo test --all

# View benchmark reports
open target/criterion/report/index.html
```

## Introspection

```rust
use component_framework::channel::kanal_bounded::KanalChannel;
use component_framework::iunknown::IUnknown;

let ch = KanalChannel::<u32>::new(256);
assert_eq!(ch.version(), "1.0.0");
assert_eq!(ch.provided_interfaces().len(), 2);
assert_eq!(ch.provided_interfaces()[0].name, "ISender");
assert_eq!(ch.provided_interfaces()[1].name, "IReceiver");
assert!(ch.receptacles().is_empty());
```
