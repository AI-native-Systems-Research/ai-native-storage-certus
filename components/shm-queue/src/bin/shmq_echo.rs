//! Cross-process smoke + latency harness for the `shm-queue` transport.
//!
//! Proves `MAP_SHARED` + shared-futex correctness across two real processes and
//! measures bare round-trip latency (no dispatcher, no CUDA — just the queue).
//!
//! ```text
//! # terminal 1: start an echo server (8 channels, 1 MiB req / 128 KiB resp)
//! shmq-echo serve /dev/shm/certus-shmq-echo 8 1048576 131072
//!
//! # terminal 2: 100k round-trips of a 256-byte payload
//! shmq-echo bench /dev/shm/certus-shmq-echo 100000 256
//! ```

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use shm_queue::{Client, Server};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let usage = || -> ! {
        eprintln!(
            "usage:\n  {0} serve <path> [n_channels=8] [cap_req=1048576] [cap_resp=131072]\n  \
             {0} bench <path> [iters=100000] [payload_bytes=256]",
            args.first().map(String::as_str).unwrap_or("shmq-echo")
        );
        std::process::exit(2);
    };
    let mode = args.get(1).map(String::as_str).unwrap_or_else(|| usage());
    let path = args.get(2).cloned().unwrap_or_else(|| usage());

    match mode {
        "serve" => {
            let n = parse_or(&args, 3, 8);
            let cap_req = parse_or(&args, 4, 1 << 20);
            let cap_resp = parse_or(&args, 5, 1 << 17);
            serve(&path, n, cap_req, cap_resp);
        }
        "bench" => {
            let iters = parse_or(&args, 3, 100_000);
            let payload = parse_or(&args, 4, 256);
            bench(&path, iters, payload);
        }
        _ => usage(),
    }
}

fn parse_or(args: &[String], idx: usize, default: usize) -> usize {
    args.get(idx)
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn serve(path: &str, n: usize, cap_req: usize, cap_resp: usize) {
    let server = Server::create(path, n, cap_req, cap_resp).expect("create shmq");
    eprintln!(
        "[shmq-echo] serving {path} channels={n} cap_req={cap_req} cap_resp={cap_resp} (Ctrl-C to stop)"
    );
    let mut last_seen = server.seq_baseline();
    let mut sweeps: u64 = 0;
    loop {
        for (ch, seen) in last_seen.iter_mut().enumerate() {
            if let Some(req) = server.take_request(ch, seen) {
                // Echo payload back, status = opcode.
                server.reply(req.channel, req.seq, req.opcode, &req.payload);
            }
        }
        sweeps = sweeps.wrapping_add(1);
        if sweeps % 4096 == 0 {
            server.heartbeat();
        }
        std::hint::spin_loop();
    }
}

fn bench(path: &str, iters: usize, payload: usize) {
    let client = Client::attach(path, Duration::from_secs(10)).expect("attach shmq");
    let ch = client.claim_channel().expect("claim channel");
    let msg = vec![0xABu8; payload];
    let mut latencies: Vec<u64> = Vec::with_capacity(iters);

    // Warmup.
    for _ in 0..1000.min(iters) {
        client
            .request(
                ch,
                0,
                &msg,
                500,
                Duration::from_millis(100),
                Duration::from_secs(5),
            )
            .expect("warmup rtt");
    }

    let t0 = Instant::now();
    for _ in 0..iters {
        let s = Instant::now();
        let (_status, resp) = client
            .request(
                ch,
                0,
                &msg,
                500,
                Duration::from_millis(100),
                Duration::from_secs(5),
            )
            .expect("rtt");
        debug_assert_eq!(resp.len(), payload);
        latencies.push(s.elapsed().as_nanos() as u64);
    }
    let total = t0.elapsed();
    client.release_channel(ch);

    latencies.sort_unstable();
    let pct = |p: f64| latencies[((latencies.len() as f64 * p) as usize).min(latencies.len() - 1)];
    let mean: f64 = latencies.iter().sum::<u64>() as f64 / latencies.len() as f64;
    println!("shmq round-trip over {iters} iters, {payload}B payload:");
    println!(
        "  throughput : {:.0} req/s",
        iters as f64 / total.as_secs_f64()
    );
    println!("  mean       : {:.2} µs", mean / 1000.0);
    println!("  p50        : {:.2} µs", pct(0.50) as f64 / 1000.0);
    println!("  p99        : {:.2} µs", pct(0.99) as f64 / 1000.0);
    println!("  p999       : {:.2} µs", pct(0.999) as f64 / 1000.0);
    println!(
        "  max        : {:.2} µs",
        *latencies.last().unwrap() as f64 / 1000.0
    );
    let _ = Ordering::Relaxed;
}
