use std::sync::Arc;

use interfaces::IMemoryTier;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

pub async fn serve_metrics(port: u16, mt: Arc<dyn IMemoryTier + Send + Sync>) {
    let listener = match TcpListener::bind(("0.0.0.0", port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("metrics: failed to bind port {port}: {e}");
            return;
        }
    };
    eprintln!("metrics: listening on 0.0.0.0:{port}");

    loop {
        let (mut stream, _) = match listener.accept().await {
            Ok(conn) => conn,
            Err(_) => continue,
        };

        let body = render_metrics(&*mt);

        let (reader, mut writer) = stream.split();
        let mut buf_reader = BufReader::new(reader);
        let mut request_line = String::new();
        if buf_reader.read_line(&mut request_line).await.is_err() {
            continue;
        }

        let response = if request_line.starts_with("GET /metrics") {
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
        } else {
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
        };

        let _ = writer.write_all(response.as_bytes()).await;
    }
}

fn render_metrics(mt: &dyn IMemoryTier) -> String {
    let snap = mt.telemetry_snapshot();
    let capacity = mt.capacity();
    let used = mt.used();
    let free = capacity.saturating_sub(used);

    format!(
        "# HELP certus_memory_tier_evictions_total Total LRU evictions\n\
         # TYPE certus_memory_tier_evictions_total counter\n\
         certus_memory_tier_evictions_total {}\n\
         # HELP certus_memory_tier_write_lock_contentions_total Write-lock contention events\n\
         # TYPE certus_memory_tier_write_lock_contentions_total counter\n\
         certus_memory_tier_write_lock_contentions_total {}\n\
         # HELP certus_memory_tier_read_lock_contentions_total Read-lock contention events\n\
         # TYPE certus_memory_tier_read_lock_contentions_total counter\n\
         certus_memory_tier_read_lock_contentions_total {}\n\
         # HELP certus_memory_tier_capacity_bytes Total pool capacity in bytes\n\
         # TYPE certus_memory_tier_capacity_bytes gauge\n\
         certus_memory_tier_capacity_bytes {}\n\
         # HELP certus_memory_tier_used_bytes Bytes currently allocated\n\
         # TYPE certus_memory_tier_used_bytes gauge\n\
         certus_memory_tier_used_bytes {}\n\
         # HELP certus_memory_tier_free_bytes Bytes available for allocation\n\
         # TYPE certus_memory_tier_free_bytes gauge\n\
         certus_memory_tier_free_bytes {}\n",
        snap.evictions,
        snap.write_lock_contentions,
        snap.read_lock_contentions,
        capacity,
        used,
        free,
    )
}
