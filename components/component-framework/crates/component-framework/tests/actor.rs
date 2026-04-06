//! Integration tests for actor lifecycle and IUnknown component model.

use component_framework::actor::{Actor, ActorError, ActorHandler};
use component_framework::channel::ISender;
use component_framework::iunknown::{query, IUnknown};
use std::sync::{Arc, Mutex};
use std::thread;

struct RecordHandler {
    log: Arc<Mutex<Vec<String>>>,
    thread_id: Arc<Mutex<Option<thread::ThreadId>>>,
}

impl ActorHandler<String> for RecordHandler {
    fn handle(&mut self, msg: String) {
        *self.thread_id.lock().unwrap() = Some(thread::current().id());
        self.log.lock().unwrap().push(msg);
    }

    fn on_start(&mut self) {
        self.log.lock().unwrap().push("START".into());
    }

    fn on_stop(&mut self) {
        self.log.lock().unwrap().push("STOP".into());
    }
}

#[test]
fn actor_full_lifecycle() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let tid = Arc::new(Mutex::new(None));

    let actor = Actor::new(
        RecordHandler {
            log: log.clone(),
            thread_id: tid.clone(),
        },
        |_| {},
    );

    assert!(!actor.is_active());

    let handle = actor.activate().unwrap();
    assert!(actor.is_active());

    handle.send("hello".into()).unwrap();
    handle.send("world".into()).unwrap();

    handle.deactivate().unwrap();
    assert!(!actor.is_active());

    let log = log.lock().unwrap();
    assert_eq!(log[0], "START");
    assert_eq!(log[1], "hello");
    assert_eq!(log[2], "world");
    assert_eq!(log[3], "STOP");

    // Verify different thread
    let actor_tid = tid.lock().unwrap().unwrap();
    assert_ne!(actor_tid, thread::current().id());
}

#[test]
fn actor_double_activate_error() {
    struct Noop;
    impl ActorHandler<()> for Noop {
        fn handle(&mut self, _: ()) {}
    }

    let actor = Actor::new(Noop, |_| {});
    let handle = actor.activate().unwrap();
    assert_eq!(actor.activate().unwrap_err(), ActorError::AlreadyActive);
    handle.deactivate().unwrap();
}

#[test]
fn actor_panic_recovery() {
    let panics = Arc::new(Mutex::new(Vec::new()));
    let results = Arc::new(Mutex::new(Vec::new()));

    struct PanicOnFive {
        results: Arc<Mutex<Vec<u32>>>,
    }

    impl ActorHandler<u32> for PanicOnFive {
        fn handle(&mut self, msg: u32) {
            if msg == 5 {
                panic!("five is bad");
            }
            self.results.lock().unwrap().push(msg);
        }
    }

    let panics_clone = panics.clone();
    let actor = Actor::new(
        PanicOnFive {
            results: results.clone(),
        },
        move |payload| {
            let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                s.to_string()
            } else {
                "unknown".into()
            };
            panics_clone.lock().unwrap().push(msg);
        },
    );

    let handle = actor.activate().unwrap();
    for i in 0..10 {
        handle.send(i).unwrap();
    }
    handle.deactivate().unwrap();

    let results = results.lock().unwrap();
    assert_eq!(*results, vec![0, 1, 2, 3, 4, 6, 7, 8, 9]); // 5 skipped

    let panics = panics.lock().unwrap();
    assert_eq!(panics.len(), 1);
    assert!(panics[0].contains("five is bad"));
}

#[test]
fn actor_no_resource_leak() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static ACTIVE_COUNT: AtomicUsize = AtomicUsize::new(0);

    struct Tracked;

    impl ActorHandler<()> for Tracked {
        fn handle(&mut self, _: ()) {}
        fn on_start(&mut self) {
            ACTIVE_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        fn on_stop(&mut self) {
            ACTIVE_COUNT.fetch_sub(1, Ordering::Relaxed);
        }
    }

    ACTIVE_COUNT.store(0, Ordering::Relaxed);

    let actor = Actor::new(Tracked, |_| {});
    let handle = actor.activate().unwrap();
    thread::sleep(std::time::Duration::from_millis(10));
    assert_eq!(ACTIVE_COUNT.load(Ordering::Relaxed), 1);

    handle.deactivate().unwrap();
    assert_eq!(ACTIVE_COUNT.load(Ordering::Relaxed), 0);
}

// --- IUnknown integration tests ---

#[test]
fn actor_iunknown_query_isender() {
    let log = Arc::new(Mutex::new(Vec::new()));

    struct LogHandler {
        log: Arc<Mutex<Vec<u32>>>,
    }
    impl ActorHandler<u32> for LogHandler {
        fn handle(&mut self, msg: u32) {
            self.log.lock().unwrap().push(msg);
        }
    }

    let actor = Actor::new(LogHandler { log: log.clone() }, |_| {});

    // Query ISender<u32> via IUnknown before activation
    let sender: Arc<dyn ISender<u32> + Send + Sync> =
        query::<dyn ISender<u32> + Send + Sync>(&actor).unwrap();

    let handle = actor.activate().unwrap();
    sender.send(100).unwrap();
    sender.send(200).unwrap();
    handle.deactivate().unwrap();

    assert_eq!(*log.lock().unwrap(), vec![100, 200]);
}

#[test]
fn actor_iunknown_isender_and_handle_coexist() {
    let log = Arc::new(Mutex::new(Vec::new()));

    struct LogHandler {
        log: Arc<Mutex<Vec<u32>>>,
    }
    impl ActorHandler<u32> for LogHandler {
        fn handle(&mut self, msg: u32) {
            self.log.lock().unwrap().push(msg);
        }
    }

    let actor = Actor::new(LogHandler { log: log.clone() }, |_| {});
    let sender: Arc<dyn ISender<u32> + Send + Sync> =
        query::<dyn ISender<u32> + Send + Sync>(&actor).unwrap();

    let handle = actor.activate().unwrap();

    // Send via handle and ISender concurrently
    handle.send(1).unwrap();
    sender.send(2).unwrap();
    handle.send(3).unwrap();
    sender.send(4).unwrap();

    handle.deactivate().unwrap();

    let log = log.lock().unwrap();
    assert_eq!(log.len(), 4);
    // All messages received (order depends on scheduling)
    assert!(log.contains(&1));
    assert!(log.contains(&2));
    assert!(log.contains(&3));
    assert!(log.contains(&4));
}

#[test]
fn actor_iunknown_introspection() {
    struct Noop;
    impl ActorHandler<u32> for Noop {
        fn handle(&mut self, _: u32) {}
    }

    let actor = Actor::new(Noop, |_| {});

    assert_eq!(actor.version(), "1.0.0");
    assert_eq!(actor.provided_interfaces().len(), 1);
    assert_eq!(actor.provided_interfaces()[0].name, "ISender");
    assert!(actor.receptacles().is_empty());
}

#[test]
fn actor_iunknown_isender_from_different_threads() {
    let log = Arc::new(Mutex::new(Vec::new()));

    struct LogHandler {
        log: Arc<Mutex<Vec<u32>>>,
    }
    impl ActorHandler<u32> for LogHandler {
        fn handle(&mut self, msg: u32) {
            self.log.lock().unwrap().push(msg);
        }
    }

    let actor = Actor::new(LogHandler { log: log.clone() }, |_| {});
    let sender: Arc<dyn ISender<u32> + Send + Sync> =
        query::<dyn ISender<u32> + Send + Sync>(&actor).unwrap();

    let handle = actor.activate().unwrap();

    // Send from multiple threads using the IUnknown-queried sender
    let sender2 = Arc::clone(&sender);
    let t1 = thread::spawn(move || {
        for i in 0..50 {
            sender2.send(i).unwrap();
        }
    });

    for i in 50..100 {
        handle.send(i).unwrap();
    }

    t1.join().unwrap();
    handle.deactivate().unwrap();

    let log = log.lock().unwrap();
    assert_eq!(log.len(), 100);
}

// --- Registry integration tests (FR-018: actors registerable in ComponentRegistry) ---

#[test]
fn actor_registerable_in_component_registry() {
    use component_framework::component_ref::ComponentRef;
    use component_framework::error::RegistryError;
    use component_framework::registry::ComponentRegistry;
    use std::any::Any;

    let log = Arc::new(Mutex::new(Vec::new()));

    struct LogHandler {
        log: Arc<Mutex<Vec<String>>>,
    }
    impl ActorHandler<String> for LogHandler {
        fn handle(&mut self, msg: String) {
            self.log.lock().unwrap().push(msg);
        }
    }

    let log_clone = log.clone();
    let registry = ComponentRegistry::new();
    registry
        .register(
            "log-actor",
            move |_config: Option<&dyn Any>| -> Result<ComponentRef, RegistryError> {
                let actor = Actor::new(
                    LogHandler {
                        log: log_clone.clone(),
                    },
                    |_| {},
                );
                Ok(ComponentRef::from(Arc::new(actor)))
            },
        )
        .unwrap();

    // Create actor via registry
    let comp = registry.create("log-actor", None).unwrap();

    // Verify introspection works through ComponentRef
    assert_eq!(comp.version(), "1.0.0");
    assert!(!comp.provided_interfaces().is_empty());

    // Query ISender<String> through IUnknown
    let sender: Arc<dyn ISender<String> + Send + Sync> =
        query::<dyn ISender<String> + Send + Sync>(&*comp).unwrap();

    // Verify the sender is functional (messages queue even without activation)
    assert!(sender.try_send("hello".into()).is_ok());
}

// --- Third-party wiring via IUnknown (FR-019) ---

#[test]
fn actor_channel_wiring_via_iunknown_query() {
    use component_framework::channel::spsc::SpscChannel;
    use component_framework::channel::IReceiver;

    let log = Arc::new(Mutex::new(Vec::<String>::new()));

    struct CollectorHandler {
        log: Arc<Mutex<Vec<String>>>,
    }
    impl ActorHandler<String> for CollectorHandler {
        fn handle(&mut self, msg: String) {
            self.log.lock().unwrap().push(msg);
        }
    }

    // Create actor (keep concrete ref for activate/deactivate) and channel as IUnknown
    let actor = Actor::simple(CollectorHandler { log: log.clone() });
    let channel: Arc<dyn IUnknown> = Arc::new(SpscChannel::<String>::new(16));

    // Third-party discovery: query ISender from actor via IUnknown trait
    let actor_sender: Arc<dyn ISender<String> + Send + Sync> =
        query::<dyn ISender<String> + Send + Sync>(&actor).unwrap();

    // Third-party discovery: query ISender and IReceiver from channel via IUnknown
    let chan_sender: Arc<dyn ISender<String> + Send + Sync> =
        query::<dyn ISender<String> + Send + Sync>(&*channel).unwrap();
    let chan_receiver: Arc<dyn IReceiver<String> + Send + Sync> =
        query::<dyn IReceiver<String> + Send + Sync>(&*channel).unwrap();

    // Verify introspection works on both (third-party can enumerate capabilities)
    let actor_iunknown: &dyn IUnknown = &actor;
    assert!(actor_iunknown
        .provided_interfaces()
        .iter()
        .any(|i| i.name == "ISender"));
    assert!(channel
        .provided_interfaces()
        .iter()
        .any(|i| i.name == "ISender"));
    assert!(channel
        .provided_interfaces()
        .iter()
        .any(|i| i.name == "IReceiver"));

    // Send a message through the channel (discovered via IUnknown)
    chan_sender.send("hello via IUnknown".into()).unwrap();
    let msg = chan_receiver.recv().unwrap();
    assert_eq!(msg, "hello via IUnknown");

    // Activate actor so it processes messages, then send via discovered ISender
    let handle = actor.activate().unwrap();
    actor_sender.send("direct to actor".into()).unwrap();
    drop(actor_sender);
    handle.deactivate().unwrap();

    let received = log.lock().unwrap();
    assert!(received.contains(&"direct to actor".to_string()));
}

// --- Channel components registerable in registry (FR-018) ---

#[test]
fn channel_registerable_in_component_registry() {
    use component_framework::channel::spsc::SpscChannel;
    use component_framework::component_ref::ComponentRef;
    use component_framework::error::RegistryError;
    use component_framework::registry::ComponentRegistry;
    use std::any::Any;

    let registry = ComponentRegistry::new();
    registry
        .register(
            "spsc-string",
            |_config: Option<&dyn Any>| -> Result<ComponentRef, RegistryError> {
                Ok(ComponentRef::new(
                    Arc::new(SpscChannel::<String>::new(1024)) as Arc<dyn IUnknown>,
                ))
            },
        )
        .unwrap();

    // Create channel via registry
    let comp = registry.create("spsc-string", None).unwrap();

    // Verify introspection
    assert_eq!(comp.version(), "1.0.0");
    assert!(comp
        .provided_interfaces()
        .iter()
        .any(|i| i.name == "ISender"));
    assert!(comp
        .provided_interfaces()
        .iter()
        .any(|i| i.name == "IReceiver"));

    // Query ISender through IUnknown
    let sender: Arc<dyn ISender<String> + Send + Sync> =
        query::<dyn ISender<String> + Send + Sync>(&*comp).unwrap();
    assert!(sender.try_send("hello".into()).is_ok());
}
