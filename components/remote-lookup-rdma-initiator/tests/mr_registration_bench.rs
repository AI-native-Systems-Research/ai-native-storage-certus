//! Hardware measurement: cost of `ibv_reg_mr` as a function of pool size.
//!
//! This is the input to the **single-MR decision** — whether the responder
//! (which registers the whole memory-tier pool once at `initialize`) and the
//! initiator (which today re-registers the whole pool *per connection*, see
//! `src/connection.rs` `RealTransport::connect`) should share one MR/PD instead
//! of each registering the same pool. That trade-off hinges on how expensive a
//! full-pool `ibv_reg_mr` is and how it scales with pool size: if it is a large,
//! size-scaling fraction of connect latency, sharing pays off; if it is
//! negligible, the simpler per-connection registration is fine.
//!
//! It isolates the registration primitive: open the first RDMA device, allocate
//! a PD once, then for a sweep of buffer sizes allocate a resident (pre-faulted)
//! host buffer and time `ibv_reg_mr` (LOCAL_WRITE | REMOTE_WRITE — the
//! responder's landing-MR access) and the matching `ibv_dereg_mr`. No CM
//! connect, no data path.
//!
//! Registration cost is dominated by pinning the buffer's pages, so it scales
//! with **page count**. The `CERTUS_MR_BENCH_HUGE=1` mode allocates the buffer
//! from hugepages (`mmap(MAP_HUGETLB)`, default hugepage size), which is what a
//! production DMA/memory-tier pool typically uses — far fewer page-table entries
//! than ordinary 4 KiB pages, so it registers proportionally faster. Compare the
//! two modes to decide whether the per-connection re-registration is actually
//! worth eliminating on the real pool backing.
//!
//! # Running (on a host with an RDMA device)
//!
//! ```bash
//! # 4 KiB pages (upper bound):
//! cargo test -p remote-lookup-rdma-initiator --features rdma \
//!   -- --ignored --nocapture mr_registration_cost
//! # Hugepages (needs a hugepage pool; sizes are rounded up to the hugepage size):
//! CERTUS_MR_BENCH_HUGE=1 CERTUS_MR_BENCH_SIZES_MIB=1024,2048,4096 \
//!   cargo test -p remote-lookup-rdma-initiator --features rdma \
//!     -- --ignored --nocapture mr_registration_cost
//! ```
#![cfg(feature = "rdma")]

use std::ffi::c_void;
use std::fs;
use std::os::raw::c_int;
use std::ptr;
use std::time::Instant;

use remote_lookup_rdma_initiator::ffi;

// mmap flags/prot (Linux x86-64).
const PROT_READ_WRITE: c_int = 0x1 | 0x2;
const MAP_PRIVATE: c_int = 0x02;
const MAP_ANONYMOUS: c_int = 0x20;
const MAP_HUGETLB: c_int = 0x40000;
const MAP_FAILED: *mut c_void = usize::MAX as *mut c_void; // (void*)-1

extern "C" {
    fn mmap(
        addr: *mut c_void,
        length: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(addr: *mut c_void, length: usize) -> c_int;
}

/// Default hugepage size in bytes (from `/proc/meminfo` `Hugepagesize`), or
/// 2 MiB if it cannot be read.
fn huge_page_bytes() -> usize {
    fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("Hugepagesize:"))
                .and_then(|v| v.trim().strip_suffix("kB"))
                .and_then(|kb| kb.trim().parse::<usize>().ok())
                .map(|kb| kb * 1024)
        })
        .unwrap_or(2 * 1024 * 1024)
}

fn use_huge() -> bool {
    matches!(
        std::env::var("CERTUS_MR_BENCH_HUGE").as_deref(),
        Ok("1") | Ok("true")
    )
}

/// The resident buffer to register: a page-touched anonymous mapping. Freed via
/// `munmap` on drop so each sweep size is measured against a fresh mapping.
struct Buf {
    ptr: *mut c_void,
    len: usize,
}

impl Buf {
    /// Map `len` bytes (rounded up to the page granularity), fault every page in
    /// so registration measures pinning of resident pages (as a real pool is),
    /// then return the mapping. `None` if the mapping fails (e.g. too few free
    /// hugepages).
    unsafe fn resident(len: usize, huge: bool) -> Option<Buf> {
        let (gran, extra) = if huge {
            (huge_page_bytes(), MAP_HUGETLB)
        } else {
            (4096, 0)
        };
        let mapped = len.div_ceil(gran) * gran;
        let p = mmap(
            ptr::null_mut(),
            mapped,
            PROT_READ_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS | extra,
            -1,
            0,
        );
        if p == MAP_FAILED {
            return None;
        }
        // Fault one byte per page granularity.
        let mut off = 0;
        while off < mapped {
            ptr::write_volatile(p.cast::<u8>().add(off), 1);
            off += gran;
        }
        Some(Buf {
            ptr: p,
            len: mapped,
        })
    }
}

impl Drop for Buf {
    fn drop(&mut self) {
        // SAFETY: ptr/len came from a successful mmap of this length.
        unsafe {
            munmap(self.ptr, self.len);
        }
    }
}

/// Open the first RDMA device that opens; returns its context (never closed —
/// the process exits at test end). `None` if none is present.
unsafe fn open_first_device() -> Option<*mut ffi::ibv_context> {
    let mut n: c_int = 0;
    let list = ffi::ibv_get_device_list(&mut n);
    if list.is_null() || n == 0 {
        return None;
    }
    let mut ctx = ptr::null_mut();
    for i in 0..n as isize {
        let dev = *list.offset(i);
        if dev.is_null() {
            continue;
        }
        let c = ffi::ibv_open_device(dev);
        if !c.is_null() {
            ctx = c;
            break;
        }
    }
    ffi::ibv_free_device_list(list);
    if ctx.is_null() {
        None
    } else {
        Some(ctx)
    }
}

fn sizes_mib() -> Vec<usize> {
    match std::env::var("CERTUS_MR_BENCH_SIZES_MIB") {
        Ok(s) => s
            .split(',')
            .filter_map(|t| t.trim().parse::<usize>().ok())
            .filter(|&m| m > 0)
            .collect(),
        Err(_) => vec![16, 64, 256, 1024],
    }
}

fn iters() -> usize {
    std::env::var("CERTUS_MR_BENCH_ITERS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(5)
}

#[test]
#[ignore = "requires an RDMA device; measures ibv_reg_mr cost vs pool size"]
fn mr_registration_cost() {
    let huge = use_huge();
    // SAFETY: single-threaded test; all pointers are checked before use and the
    // device context outlives the measurement (process exits at test end).
    unsafe {
        let Some(ctx) = open_first_device() else {
            eprintln!("mr_registration_cost: no RDMA device — skipping");
            return;
        };
        let pd = ffi::ibv_alloc_pd(ctx);
        assert!(!pd.is_null(), "ibv_alloc_pd failed");
        let access = ffi::IBV_ACCESS_LOCAL_WRITE | ffi::IBV_ACCESS_REMOTE_WRITE;

        let n = iters();
        let page_desc = if huge {
            format!("{} MiB hugepages", huge_page_bytes() / (1024 * 1024))
        } else {
            "4 KiB pages".to_string()
        };
        println!(
            "\n  ibv_reg_mr cost ({page_desc}, LOCAL_WRITE|REMOTE_WRITE, median of {n} iters)"
        );
        println!(
            "  {:>9} | {:>12} | {:>12} | {:>10}",
            "pool", "reg median", "reg min", "reg GiB/s"
        );
        println!("  {:-<9}-+-{:-<12}-+-{:-<12}-+-{:-<10}", "", "", "", "");

        for mib in sizes_mib() {
            let len = mib * 1024 * 1024;
            let Some(buf) = Buf::resident(len, huge) else {
                println!("  {mib:>7} MiB | mmap failed (insufficient free hugepages?)");
                continue;
            };

            let mut reg_us: Vec<u128> = Vec::with_capacity(n);
            for _ in 0..n {
                let t0 = Instant::now();
                let mr = ffi::ibv_reg_mr(pd, buf.ptr, buf.len, access);
                let dt = t0.elapsed().as_micros();
                assert!(!mr.is_null(), "ibv_reg_mr failed for {mib} MiB");
                reg_us.push(dt);
                let rc = ffi::ibv_dereg_mr(mr);
                assert_eq!(rc, 0, "ibv_dereg_mr failed");
            }
            reg_us.sort_unstable();
            let median = reg_us[n / 2];
            let min = reg_us[0];
            let gib_s = (buf.len as f64 / (1u64 << 30) as f64) / (median as f64 / 1e6);
            println!(
                "  {:>7} MiB | {:>9} us | {:>9} us | {:>8.2}",
                mib, median, min, gib_s
            );
        }
        println!();

        let rc = ffi::ibv_dealloc_pd(pd);
        assert_eq!(rc, 0, "ibv_dealloc_pd failed");
    }
}
