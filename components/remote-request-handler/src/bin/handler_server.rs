//! Standalone handler server for testing the remote-request-handler RDMA path.
//!
//! Listens for connections, processes handshakes, handles batched lookups
//! (resolving via a stub dispatcher that always returns "not found"),
//! and handles close requests.

use anyhow::Result;
use clap::Parser;

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

    println!(
        "Remote Request Handler Server (standalone)\n\
         ===========================================\n\
         Listening on: {}:{}\n\
         Dispatcher:   none (all lookups return not-found)\n",
        args.addr, args.port
    );

    remote_request_handler::serve::run_blocking(&args.addr, args.port, None)
}
