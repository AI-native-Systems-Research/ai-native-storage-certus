//! Repair test for BDK-CMD-ASYNC-TIMEOUT-CANCELS: a timed-out async op must
//! get an `AsyncCancel` submitted to io_uring even when the submission queue
//! is full at the moment the timeout is detected.

use super::*;

use std::time::Duration;

/// user_data of the filler SQEs: internal bit set, so the actor ignores their CQEs.
const FILLER_UD: u64 = 1 << 63;

fn temp_backing_file() -> (std::path::PathBuf, OwnedFd) {
    let path = std::env::temp_dir().join(format!(
        "bdk-repair-timeout-cancel-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .expect("create backing file");
    file.set_len(4096).expect("size backing file");
    (path, OwnedFd::from(file))
}

/// `None` when the kernel refuses io_uring (e.g. `kernel.io_uring_disabled=2`, the RHEL default).
fn handler(fd: OwnedFd, path: std::path::PathBuf) -> Option<KernelHandler> {
    let config = DeviceConfig::new(path, 512, 8).expect("config");
    let ring = match IoUring::new(2) {
        Ok(ring) => ring,
        Err(e) => {
            eprintln!("SKIPPED: io_uring unavailable on this machine ({e})");
            return None;
        }
    };
    #[cfg(not(feature = "telemetry"))]
    {
        Some(KernelHandler::new(fd, config, ring, None))
    }
    #[cfg(feature = "telemetry")]
    {
        Some(KernelHandler::with_telemetry(
            fd,
            config,
            ring,
            None,
            Arc::new(TelemetryStats::new()),
        ))
    }
}

#[test]
fn timeout_with_full_submission_queue_still_submits_async_cancel() {
    let (path, fd) = temp_backing_file();
    let Some(mut h) = handler(fd, path.clone()) else {
        let _ = std::fs::remove_file(&path);
        return;
    };

    // An async op (handle 1) whose deadline has already passed.
    let now = Instant::now();
    h.inflight.insert(
        1,
        InflightOp {
            handle: OpHandle(1),
            client_id: 7,
            deadline: Some(now),
            is_read: true,
            #[cfg(feature = "telemetry")]
            start: now,
            #[cfg(feature = "telemetry")]
            bytes: 512,
        },
    );

    // Fill the submission queue without submitting.
    let mut fillers = 0u64;
    loop {
        let nop = opcode::Nop::new().build().user_data(FILLER_UD | fillers);
        // SAFETY: a NOP SQE references no memory.
        if unsafe { h.ring.submission().push(&nop) }.is_err() {
            break;
        }
        fillers += 1;
    }
    assert!(fillers > 0, "submission queue had no slots");

    h.check_timeouts();
    assert!(
        !h.inflight.contains_key(&1),
        "timed-out op must leave the inflight map"
    );

    // Collect CQEs: the AsyncCancel (user_data 0) for handle 1 must show up.
    let mut saw_cancel = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    while !saw_cancel && Instant::now() < deadline {
        let _ = h.ring.submit();
        for cqe in h.ring.completion() {
            if cqe.user_data() == 0 {
                saw_cancel = true;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = std::fs::remove_file(&path);

    assert!(
        saw_cancel,
        "no AsyncCancel was submitted to io_uring for the timed-out op (submission queue was full)"
    );
}
