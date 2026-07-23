# logger (v1)

**Crate**: `logger`
**Path**: `components/logger/`
**Version**: 0.1.0

## Description

Production logger component. Writes timestamped, level-filtered log lines to stderr (default) or a file. Timestamps are ISO 8601 UTC with milliseconds. Log level is controlled by the `RUST_LOG` environment variable. Color output is auto-detected via `libc::isatty`.

## Component Definition

```
LoggerComponent {
    version: "0.1.0",
    provides: [ILogger],
}
```

## Interface Definition

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

## Verified Properties

None. No formal verification model exists for this component.

## Receptacles

None.

## Log Levels

`Error` (0) > `Warn` (1) > `Info` (2) > `Debug` (3). Parsed from `RUST_LOG` env var.

## Constructors

- `new_default()` — stderr output with auto-detected ANSI colors (`libc::isatty`)
- `new_with_file(path)` — file output in append mode, no colors
- `new_with_writer(writer, level, use_color)` — arbitrary writer (for testing)

## Key Design Decisions

- **Thread-safe**: `Mutex<Box<dyn Write + Send>>` wraps the output writer.
- **ISO 8601 UTC timestamps** with millisecond precision.
- **ANSI color**: Error=red, Warn=orange, Info=green, Debug=cyan (disabled for non-TTY/file output).
