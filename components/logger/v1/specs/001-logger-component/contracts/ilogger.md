# ILogger Interface Contract

**Crate**: `interfaces`
**File**: `src/ilogger.rs`
**Defined via**: `define_interface!`

## Trait Definition

```rust
define_interface! {
    pub ILogger {
        fn error(&self, msg: &str);
        fn warn(&self, msg: &str);
        fn info(&self, msg: &str);
        fn debug(&self, msg: &str);
    }
}
```

## Method Contracts

### `fn error(&self, msg: &str)`

Log a message at Error level.

- **Precondition**: None (always callable).
- **Postcondition**: If configured level >= Error (always true),
  writes `{timestamp} ERROR {msg}\n` to the output destination.
- **Thread safety**: Safe to call concurrently from multiple threads.

### `fn warn(&self, msg: &str)`

Log a message at Warn level.

- **Precondition**: None.
- **Postcondition**: If configured level >= Warn, writes
  `{timestamp} WARN  {msg}\n` to the output destination.
- **Thread safety**: Safe to call concurrently.

### `fn info(&self, msg: &str)`

Log a message at Info level.

- **Precondition**: None.
- **Postcondition**: If configured level >= Info, writes
  `{timestamp} INFO  {msg}\n` to the output destination.
- **Thread safety**: Safe to call concurrently.

### `fn debug(&self, msg: &str)`

Log a message at Debug level.

- **Precondition**: None.
- **Postcondition**: If configured level >= Debug, writes
  `{timestamp} DEBUG {msg}\n` to the output destination.
- **Thread safety**: Safe to call concurrently.

## Output Format

```text
{ISO8601_TIMESTAMP} {LEVEL} {message}
```

- Timestamp: `chrono::Utc::now()` formatted as RFC 3339 with
  millisecond precision (e.g., `2026-04-17T14:30:00.123Z`)
- Level: uppercase, padded to 5 characters for alignment
- Console: ANSI color codes wrap `{LEVEL}` when stderr is a TTY
- File: no ANSI codes

## Component Integration

```rust
// Query ILogger from LoggerComponentV1
let logger = query_interface!(component, ILogger).unwrap();
logger.info("system ready");

// Bind to a receptacle
consumer.connect_receptacle_raw("logger", &*logger_component)?;
```
