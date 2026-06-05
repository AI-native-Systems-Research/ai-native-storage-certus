//! Filesystem-backed key-value storage using O_DIRECT for fair SSD comparison.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const ALIGNMENT: usize = 4096;

pub struct FsStorage {
    data_dir: PathBuf,
    index: Mutex<HashMap<u64, StoredEntry>>,
}

struct StoredEntry {
    path: PathBuf,
    size: u32,
}

impl FsStorage {
    pub fn new(data_dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(data_dir)?;
        Ok(Self {
            data_dir: data_dir.to_path_buf(),
            index: Mutex::new(HashMap::new()),
        })
    }

    pub fn format(&self) -> io::Result<()> {
        if self.data_dir.exists() {
            for entry in fs::read_dir(&self.data_dir)? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    fs::remove_file(entry.path())?;
                }
            }
        }
        self.index.lock().unwrap().clear();
        Ok(())
    }

    fn key_path(&self, key: u64) -> PathBuf {
        self.data_dir.join(format!("{:016x}.dat", key))
    }

    pub fn exists(&self, key: u64) -> bool {
        self.index.lock().unwrap().contains_key(&key)
    }

    pub fn write(&self, key: u64, data: &[u8]) -> io::Result<()> {
        let path = self.key_path(key);
        let aligned_size = (data.len() + ALIGNMENT - 1) & !(ALIGNMENT - 1);
        let mut aligned_buf = aligned_alloc(aligned_size);
        aligned_buf[..data.len()].copy_from_slice(data);

        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .custom_flags(libc::O_DIRECT | libc::O_SYNC)
            .open(&path)?;

        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        let written = unsafe {
            libc::pwrite(
                fd,
                aligned_buf.as_ptr() as *const libc::c_void,
                aligned_size,
                0,
            )
        };
        if written < 0 {
            return Err(io::Error::last_os_error());
        }

        self.index.lock().unwrap().insert(
            key,
            StoredEntry {
                path,
                size: data.len() as u32,
            },
        );
        Ok(())
    }

    pub fn read(&self, key: u64) -> io::Result<Vec<u8>> {
        let index = self.index.lock().unwrap();
        let entry = index
            .get(&key)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "key not found"))?;
        let size = entry.size as usize;
        let aligned_size = (size + ALIGNMENT - 1) & !(ALIGNMENT - 1);
        let mut aligned_buf = aligned_alloc(aligned_size);

        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECT)
            .open(&entry.path)?;

        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        let nread = unsafe {
            libc::pread(
                fd,
                aligned_buf.as_mut_ptr() as *mut libc::c_void,
                aligned_size,
                0,
            )
        };
        if nread < 0 {
            return Err(io::Error::last_os_error());
        }

        aligned_buf.truncate(size);
        Ok(aligned_buf)
    }

    pub fn remove(&self, key: u64) -> io::Result<()> {
        let mut index = self.index.lock().unwrap();
        if let Some(entry) = index.remove(&key) {
            let _ = fs::remove_file(&entry.path);
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "key not found"))
        }
    }
}

fn aligned_alloc(size: usize) -> Vec<u8> {
    unsafe {
        let mut ptr: *mut libc::c_void = std::ptr::null_mut();
        let ret = libc::posix_memalign(&mut ptr, ALIGNMENT, size);
        if ret != 0 || ptr.is_null() {
            panic!("posix_memalign failed");
        }
        std::ptr::write_bytes(ptr as *mut u8, 0, size);
        Vec::from_raw_parts(ptr as *mut u8, size, size)
    }
}
