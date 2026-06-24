//! Standalone handler server for testing the remote-request-handler RDMA path.
//!
//! Listens for connections, processes handshakes, handles batched lookups
//! (resolving via a stub dispatcher that always returns "not found"),
//! and handles close requests.

use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use interfaces::ILogger;

struct StderrLogger;

impl ILogger for StderrLogger {
    fn error(&self, msg: &str) {
        eprintln!("ERROR {msg}");
    }
    fn warn(&self, msg: &str) {
        eprintln!("WARN  {msg}");
    }
    fn info(&self, msg: &str) {
        eprintln!("INFO  {msg}");
    }
    fn debug(&self, msg: &str) {
        eprintln!("DEBUG {msg}");
    }
}

#[derive(Parser, Debug)]
#[command(name = "handler-server", about = "RDMA remote request handler server (standalone)")]
struct Args {
    /// Address to listen on.
    #[arg(long, default_value = "0.0.0.0")]
    addr: String,

    /// Port to listen on.
    #[arg(long, default_value_t = 18515)]
    port: u16,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(StderrLogger);
    logger.info(&format!(
        "Remote Request Handler Server (standalone) — {}:{}",
        args.addr, args.port
    ));

    remote_request_handler::serve::run_blocking(&args.addr, args.port, None, None, logger)
}
