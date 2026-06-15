//! Baseline filesystem-backed gRPC Dispatcher server.
//!
//! Implements the same Dispatcher proto as certus-server but uses flat files
//! for persistence and a simple in-memory HashMap as the "memory tier" cache.
//! Provides a performance baseline for comparison against the Certus stack.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use clap::Parser;
use tonic::{transport::Server, Request, Response, Status};

pub mod dispatcher_proto {
    tonic::include_proto!("certus.dispatcher.v1");
}

use dispatcher_proto::dispatcher_server::{Dispatcher, DispatcherServer};
use dispatcher_proto::*;

// --- CUDA FFI ---

#[allow(non_camel_case_types)]
type cudaIpcMemHandle_t = [u8; 64];
const CUDA_SUCCESS: i32 = 0;
const CUDA_MEMCPY_H2D: i32 = 1;
const CUDA_MEMCPY_D2H: i32 = 2;
const CUDA_IPC_MEM_LAZY_ENABLE_PEER_ACCESS: u32 = 1;

#[allow(improper_ctypes)]
extern "C" {
    fn cudaSetDevice(device: i32) -> i32;
    fn cudaIpcOpenMemHandle(
        devptr: *mut *mut std::ffi::c_void,
        handle: cudaIpcMemHandle_t,
        flags: u32,
    ) -> i32;
    fn cudaIpcCloseMemHandle(devptr: *mut std::ffi::c_void) -> i32;
    fn cudaMemcpy(
        dst: *mut std::ffi::c_void,
        src: *const std::ffi::c_void,
        count: usize,
        kind: i32,
    ) -> i32;
}

fn open_ipc_handle(handle_bytes: &[u8]) -> Result<*mut std::ffi::c_void, String> {
    if handle_bytes.len() != 64 {
        return Err(format!("IPC handle must be 64 bytes, got {}", handle_bytes.len()));
    }
    let mut handle = [0u8; 64];
    handle.copy_from_slice(handle_bytes);
    let mut dev_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    let err = unsafe {
        cudaIpcOpenMemHandle(
            &mut dev_ptr,
            handle,
            CUDA_IPC_MEM_LAZY_ENABLE_PEER_ACCESS,
        )
    };
    if err != CUDA_SUCCESS {
        return Err(format!("cudaIpcOpenMemHandle failed: {err}"));
    }
    Ok(dev_ptr)
}

fn close_ipc_handle(dev_ptr: *mut std::ffi::c_void) {
    unsafe { cudaIpcCloseMemHandle(dev_ptr) };
}

fn cuda_memcpy_d2h(dst: &mut [u8], src: *const std::ffi::c_void, size: usize) -> Result<(), String> {
    let err = unsafe { cudaMemcpy(dst.as_mut_ptr() as *mut std::ffi::c_void, src, size, CUDA_MEMCPY_D2H) };
    if err != CUDA_SUCCESS {
        return Err(format!("cudaMemcpy D2H failed: {err}"));
    }
    Ok(())
}

fn cuda_memcpy_h2d(dst: *mut std::ffi::c_void, src: &[u8], size: usize) -> Result<(), String> {
    let err = unsafe { cudaMemcpy(dst, src.as_ptr() as *const std::ffi::c_void, size, CUDA_MEMCPY_H2D) };
    if err != CUDA_SUCCESS {
        return Err(format!("cudaMemcpy H2D failed: {err}"));
    }
    Ok(())
}

// --- File Store ---

struct LruEntry {
    data: Vec<u8>,
    access_order: u64,
}

struct FileStore {
    store_dir: PathBuf,
    cache: Mutex<HashMap<u64, LruEntry>>,
    cache_used: Mutex<usize>,
    cache_capacity: usize,
    access_counter: std::sync::atomic::AtomicU64,
}

impl FileStore {
    fn new(store_dir: PathBuf, cache_capacity: usize) -> Self {
        fs::create_dir_all(&store_dir).expect("failed to create store directory");
        Self {
            store_dir,
            cache: Mutex::new(HashMap::new()),
            cache_used: Mutex::new(0),
            cache_capacity,
            access_counter: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn key_path(&self, key: u64) -> PathBuf {
        self.store_dir.join(key.to_string())
    }

    fn populate(&self, key: u64, data: &[u8]) -> Result<(), String> {
        let path = self.key_path(key);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .custom_flags(libc::O_DSYNC)
            .open(&path)
            .map_err(|e| format!("file create failed: {e}"))?;
        file.write_all(data).map_err(|e| format!("file write failed: {e}"))?;

        // Insert into cache
        self.cache_insert(key, data.to_vec());
        Ok(())
    }

    fn lookup(&self, key: u64) -> Result<Vec<u8>, String> {
        // Check cache first
        {
            let mut cache = self.cache.lock().unwrap();
            if let Some(entry) = cache.get_mut(&key) {
                entry.access_order = self.access_counter.fetch_add(1, Ordering::Relaxed);
                return Ok(entry.data.clone());
            }
        }

        // Fall through to file
        let path = self.key_path(key);
        let mut file = fs::File::open(&path)
            .map_err(|_| format!("key not found: {key}"))?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .map_err(|e| format!("file read failed: {e}"))?;

        // Insert into cache on read
        self.cache_insert(key, data.clone());
        Ok(data)
    }

    fn exists(&self, key: u64) -> bool {
        self.key_path(key).exists()
    }

    fn remove(&self, key: u64) -> Result<(), String> {
        // Remove from cache
        {
            let mut cache = self.cache.lock().unwrap();
            if let Some(entry) = cache.remove(&key) {
                *self.cache_used.lock().unwrap() -= entry.data.len();
            }
        }
        let path = self.key_path(key);
        if path.exists() {
            fs::remove_file(&path).map_err(|e| format!("remove failed: {e}"))?;
        }
        Ok(())
    }

    fn touch(&self, key: u64) {
        let mut cache = self.cache.lock().unwrap();
        if let Some(entry) = cache.get_mut(&key) {
            entry.access_order = self.access_counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn clear_memory_tier(&self) -> u64 {
        let mut cache = self.cache.lock().unwrap();
        let count = cache.len() as u64;
        cache.clear();
        *self.cache_used.lock().unwrap() = 0;
        count
    }

    fn flush(&self) {
        // fsync the store directory
        if let Ok(dir) = fs::File::open(&self.store_dir) {
            let _ = dir.sync_all();
        }
    }

    fn cache_insert(&self, key: u64, data: Vec<u8>) {
        let data_len = data.len();
        if data_len > self.cache_capacity {
            return;
        }

        // Evict until we have space
        let mut used = self.cache_used.lock().unwrap();
        let mut cache = self.cache.lock().unwrap();

        while *used + data_len > self.cache_capacity && !cache.is_empty() {
            // Find LRU entry
            let lru_key = cache
                .iter()
                .min_by_key(|(_, e)| e.access_order)
                .map(|(&k, _)| k)
                .unwrap();
            if let Some(evicted) = cache.remove(&lru_key) {
                *used -= evicted.data.len();
            }
        }

        let order = self.access_counter.fetch_add(1, Ordering::Relaxed);
        cache.insert(key, LruEntry { data, access_order: order });
        *used += data_len;
    }
}

// --- gRPC Service ---

struct DispatcherService {
    store: Arc<FileStore>,
}

#[tonic::async_trait]
impl Dispatcher for DispatcherService {
    async fn populate(
        &self,
        request: Request<BatchPopulateRequest>,
    ) -> Result<Response<BatchPopulateResponse>, Status> {
        let req = request.into_inner();
        let store = Arc::clone(&self.store);

        let results = tokio::task::spawn_blocking(move || {
            let mut results = Vec::with_capacity(req.entries.len());
            for entry in &req.entries {
                let key = entry.key;
                let res = (|| -> Result<(), String> {
                    let ipc = entry.ipc_handle.as_ref()
                        .ok_or("missing ipc_handle")?;
                    let size = ipc.size as usize;
                    if size == 0 {
                        return Err("size must be > 0".into());
                    }
                    let dev_ptr = open_ipc_handle(&ipc.cuda_ipc_handle)?;
                    let mut buf = vec![0u8; size];
                    let copy_result = cuda_memcpy_d2h(&mut buf, dev_ptr, size);
                    close_ipc_handle(dev_ptr);
                    copy_result?;
                    store.populate(key, &buf)
                })();
                results.push(EntryResult {
                    key,
                    success: res.is_ok(),
                    error_code: if res.is_ok() { 0 } else { 1 },
                    error_message: res.err().unwrap_or_default(),
                });
            }
            results
        })
        .await
        .map_err(|e| Status::internal(format!("spawn_blocking failed: {e}")))?;

        Ok(Response::new(BatchPopulateResponse { results }))
    }

    async fn lookup(
        &self,
        request: Request<BatchLookupRequest>,
    ) -> Result<Response<BatchLookupResponse>, Status> {
        let req = request.into_inner();
        let store = Arc::clone(&self.store);

        let results = tokio::task::spawn_blocking(move || {
            let mut results = Vec::with_capacity(req.entries.len());
            for entry in &req.entries {
                let key = entry.key;
                let res = (|| -> Result<(), String> {
                    let ipc = entry.ipc_handle.as_ref()
                        .ok_or("missing ipc_handle")?;
                    let size = ipc.size as usize;
                    let data = store.lookup(key)?;
                    let copy_size = data.len().min(size);
                    let dev_ptr = open_ipc_handle(&ipc.cuda_ipc_handle)?;
                    let copy_result = cuda_memcpy_h2d(dev_ptr, &data[..copy_size], copy_size);
                    close_ipc_handle(dev_ptr);
                    copy_result
                })();
                results.push(EntryResult {
                    key,
                    success: res.is_ok(),
                    error_code: if res.is_ok() { 0 } else { 1 },
                    error_message: res.err().unwrap_or_default(),
                });
            }
            results
        })
        .await
        .map_err(|e| Status::internal(format!("spawn_blocking failed: {e}")))?;

        Ok(Response::new(BatchLookupResponse { results }))
    }

    async fn check(
        &self,
        request: Request<BatchCheckRequest>,
    ) -> Result<Response<BatchCheckResponse>, Status> {
        let req = request.into_inner();
        let store = Arc::clone(&self.store);
        let results = tokio::task::spawn_blocking(move || {
            req.keys
                .iter()
                .map(|&key| CheckResult {
                    key,
                    exists: store.exists(key),
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| Status::internal(format!("spawn_blocking failed: {e}")))?;

        Ok(Response::new(BatchCheckResponse { results }))
    }

    async fn remove(
        &self,
        request: Request<BatchRemoveRequest>,
    ) -> Result<Response<BatchRemoveResponse>, Status> {
        let req = request.into_inner();
        let store = Arc::clone(&self.store);
        let results = tokio::task::spawn_blocking(move || {
            req.keys
                .iter()
                .map(|&key| {
                    let res = store.remove(key);
                    EntryResult {
                        key,
                        success: res.is_ok(),
                        error_code: if res.is_ok() { 0 } else { 1 },
                        error_message: res.err().unwrap_or_default(),
                    }
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| Status::internal(format!("spawn_blocking failed: {e}")))?;

        Ok(Response::new(BatchRemoveResponse { results }))
    }

    async fn touch(
        &self,
        request: Request<BatchTouchRequest>,
    ) -> Result<Response<BatchTouchResponse>, Status> {
        let req = request.into_inner();
        let store = Arc::clone(&self.store);
        let results = tokio::task::spawn_blocking(move || {
            req.keys
                .iter()
                .map(|&key| {
                    store.touch(key);
                    EntryResult {
                        key,
                        success: true,
                        error_code: 0,
                        error_message: String::new(),
                    }
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| Status::internal(format!("spawn_blocking failed: {e}")))?;

        Ok(Response::new(BatchTouchResponse { results }))
    }

    async fn clear_memory_tier(
        &self,
        _request: Request<ClearMemoryTierRequest>,
    ) -> Result<Response<ClearMemoryTierResponse>, Status> {
        let store = Arc::clone(&self.store);
        let entries_cleared = tokio::task::spawn_blocking(move || store.clear_memory_tier())
            .await
            .map_err(|e| Status::internal(format!("spawn_blocking failed: {e}")))?;

        Ok(Response::new(ClearMemoryTierResponse { entries_cleared }))
    }

    async fn flush_to_ssd(
        &self,
        _request: Request<FlushToSsdRequest>,
    ) -> Result<Response<FlushToSsdResponse>, Status> {
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || store.flush())
            .await
            .map_err(|e| Status::internal(format!("spawn_blocking failed: {e}")))?;

        Ok(Response::new(FlushToSsdResponse { jobs_flushed: 0 }))
    }
}

// --- CLI + Main ---

fn parse_size(s: &str) -> Result<usize, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty size string".into());
    }
    let (num_str, multiplier) = match s.as_bytes().last() {
        Some(b'K' | b'k') => (&s[..s.len() - 1], 1024usize),
        Some(b'M' | b'm') => (&s[..s.len() - 1], 1024 * 1024),
        Some(b'G' | b'g') => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        _ => (s, 1usize),
    };
    let num: usize = num_str
        .parse()
        .map_err(|_| format!("invalid size: '{num_str}'"))?;
    num.checked_mul(multiplier)
        .ok_or_else(|| format!("size overflow: '{s}'"))
}

#[derive(Parser)]
#[command(
    name = "baseline-generalized-fs",
    about = "Filesystem-backed baseline gRPC server for Certus benchmarks"
)]
struct Cli {
    /// gRPC listen address
    #[arg(long = "listen", default_value = "0.0.0.0:50051")]
    listen: String,

    /// Directory to store cached objects as files
    #[arg(long = "store-dir", default_value = "/tmp/certus-baseline")]
    store_dir: String,

    /// In-memory cache size (e.g. 2G, 512M). Default: 2G.
    #[arg(long = "memory-tier-size", value_parser = parse_size, default_value = "2G")]
    memory_tier_size: usize,

    /// CUDA GPU device index
    #[arg(long = "gpu", default_value_t = 0)]
    gpu: i32,

    /// Clean store directory on startup
    #[arg(long = "format")]
    format: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let err = unsafe { cudaSetDevice(cli.gpu) };
    if err != CUDA_SUCCESS {
        return Err(format!("cudaSetDevice({}) failed: {err}", cli.gpu).into());
    }

    let store_dir = PathBuf::from(&cli.store_dir);
    if cli.format && store_dir.exists() {
        fs::remove_dir_all(&store_dir)?;
    }

    let store = Arc::new(FileStore::new(store_dir, cli.memory_tier_size));

    eprintln!("baseline-generalized-fs: store_dir={}", cli.store_dir);
    eprintln!(
        "baseline-generalized-fs: memory-tier-size={} MiB",
        cli.memory_tier_size / (1024 * 1024)
    );
    eprintln!("baseline-generalized-fs: gpu={}", cli.gpu);

    let svc = DispatcherService { store };
    let addr = cli.listen.parse()?;

    eprintln!("baseline-generalized-fs: listening on {addr}");

    let shutdown = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&shutdown);

    Server::builder()
        .add_service(DispatcherServer::new(svc))
        .serve_with_shutdown(addr, async move {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = sigterm.recv() => {},
            }
            flag.store(true, Ordering::Release);
        })
        .await?;

    eprintln!("baseline-generalized-fs: shutdown complete");
    Ok(())
}
