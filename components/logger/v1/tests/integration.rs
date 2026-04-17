use component_core::query_interface;
use component_framework::define_component;
use interfaces::ILogger;
use logger::{LogLevel, LoggerComponentV1};
use std::io::Write;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct TestWriter(Arc<Mutex<Vec<u8>>>);

impl Write for TestWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn test_query_interface_ilogger() {
    let component = LoggerComponentV1::new_default();
    let logger: Arc<dyn ILogger + Send + Sync> =
        query_interface!(component, ILogger).expect("ILogger query failed");
    logger.info("queried via IUnknown");
}

#[test]
fn test_query_interface_returns_some() {
    let component = LoggerComponentV1::new_default();
    let result: Option<Arc<dyn ILogger + Send + Sync>> = query_interface!(component, ILogger);
    assert!(result.is_some());
}

#[test]
fn test_version() {
    use component_core::iunknown::IUnknown;
    let component = LoggerComponentV1::new_default();
    assert_eq!(component.version(), "0.1.0");
}

#[test]
fn test_provided_interfaces() {
    use component_core::iunknown::IUnknown;
    let component = LoggerComponentV1::new_default();
    let interfaces = component.provided_interfaces();
    let names: Vec<&str> = interfaces.iter().map(|i| i.name).collect();
    assert!(names.contains(&"ILogger"), "missing ILogger in {names:?}");
    assert!(names.contains(&"IUnknown"), "missing IUnknown in {names:?}");
}

define_component! {
    pub TestConsumerComponent {
        version: "0.1.0",
        provides: [],
        receptacles: {
            logger: ILogger,
        },
    }
}

#[test]
fn test_receptacle_binding() {
    use component_core::iunknown::IUnknown;

    let logger_comp = LoggerComponentV1::new_default();
    let consumer = TestConsumerComponent::new();

    consumer
        .connect_receptacle_raw("logger", &*logger_comp)
        .expect("receptacle binding failed");

    assert!(consumer.logger.is_connected());
    let logger_ref = consumer.logger.get().expect("receptacle get failed");
    logger_ref.info("logged via receptacle");
}

#[test]
fn test_receptacle_info() {
    use component_core::iunknown::IUnknown;

    let consumer = TestConsumerComponent::new();
    let receptacles = consumer.receptacles();
    assert_eq!(receptacles.len(), 1);
    assert_eq!(receptacles[0].name, "logger");
    assert_eq!(receptacles[0].interface_name, "ILogger");
}

#[test]
fn test_concurrent_logging_no_interleave() {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let writer = TestWriter(Arc::clone(&buf));
    let component = LoggerComponentV1::new_with_writer(Box::new(writer), LogLevel::Debug, false);

    let threads: Vec<_> = (0..4)
        .map(|i| {
            let comp = Arc::clone(&component);
            std::thread::spawn(move || {
                for j in 0..100 {
                    comp.info(&format!("thread-{i}-msg-{j}"));
                }
            })
        })
        .collect();

    for t in threads {
        t.join().unwrap();
    }

    let output = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 400, "expected 400 lines, got {}", lines.len());

    for (i, line) in lines.iter().enumerate() {
        assert!(
            line.contains("INFO "),
            "line {i} missing INFO marker: {line}"
        );
        assert!(
            line.contains("thread-"),
            "line {i} missing thread marker: {line}"
        );
    }
}
