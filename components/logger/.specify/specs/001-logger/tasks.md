# Tasks

## Review Backfilled Spec
- [ ] Review generated user stories for accuracy
- [ ] Verify requirements match intended behavior
- [ ] Remove implementation notes that don't belong in spec
- [ ] Add any missing requirements
- [ ] Mark spec status as "Draft" or "Approved"

## Documentation Completeness
- [ ] Add `#![warn(missing_docs)]` to `lib.rs` to enforce doc coverage at compile time
- [ ] Add doc comments to `LoggerState` fields (`writer`, `level`, `use_color`)
- [ ] Add module-level documentation describing thread-safety guarantees
- [ ] Verify `cargo doc -p logger --no-deps` produces zero warnings

## Error Handling Improvements
- [ ] Evaluate whether silent write errors should optionally log to a fallback (e.g., eprintln)
- [ ] Document the "silent discard" policy in a code comment or module docs
- [ ] Consider adding a `last_error()` method to surface write failures for diagnostics

## Testing Gaps
- [ ] Add a test for `new_with_writer` with color enabled to verify all four color codes in output
- [ ] Add a test that verifies timestamp format with a regex (RFC 3339 pattern)
- [ ] Add a test for `LogLevel::from_env()` by setting `RUST_LOG` in the test (currently only `from_env_str` is tested)
- [ ] Add a property-based test (proptest) for `from_env_str` with arbitrary strings
- [ ] Add a benchmark for `new_with_file` construction time

## Performance
- [ ] Measure and document baseline numbers from Criterion benchmarks
- [ ] Evaluate whether `format!` allocation on every log call can be avoided (e.g., pre-format into a thread-local buffer)
- [ ] Consider `parking_lot::Mutex` as a lower-overhead alternative to `std::sync::Mutex`
- [ ] Add a benchmark comparing filtered vs unfiltered throughput ratio

## API Surface
- [ ] Consider adding `is_enabled(level: LogLevel) -> bool` for callers that want to skip expensive message formatting
- [ ] Evaluate adding a `with_context(key, value)` builder for structured metadata
- [ ] Consider a `Logger::builder()` pattern as an alternative to multiple constructor functions

## Concurrency
- [ ] Add a stress test with more threads (e.g., 16 or 32) to surface edge cases
- [ ] Measure and document mutex contention under load via benchmark results
- [ ] Evaluate whether `BufWriter` wrapping the inner writer improves throughput (trade-off: delayed flush)

## Configuration
- [ ] Support module-path-based filtering (e.g., `RUST_LOG=mymodule=debug`) for parity with `env_logger`
- [ ] Support comma-separated level directives (e.g., `RUST_LOG=warn,mymod=debug`)
- [ ] Add a `set_level(&self, level: LogLevel)` method for runtime level changes (requires `AtomicU8` or similar)

## Integration
- [ ] Add an example binary (`examples/basic_logging.rs`) demonstrating console and file usage
- [ ] Add an example showing receptacle binding from a consumer component
- [ ] Verify the logger integrates correctly with `extent-manager` and `block-device-spdk-nvme` receptacles
