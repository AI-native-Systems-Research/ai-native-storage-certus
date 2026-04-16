//! Extent metadata types and on-disk record serialization.

use crate::error::ExtentManagerError;

/// Maximum filename length in bytes.
pub const MAX_FILENAME_LEN: usize = 256;

/// Block size in bytes (4KiB).
pub const BLOCK_SIZE: usize = 4096;

/// In-memory representation of an extent's metadata.
///
/// # Examples
///
/// ```
/// use extent_manager::ExtentMetadata;
///
/// let meta = ExtentMetadata {
///     key: 42,
///     size_class: 0,
///     extent_size: 131072,
///     ns_id: 1,
///     offset_blocks: 100,
///     filename: Some("model.bin".to_string()),
///     data_crc: Some(0xDEADBEEF),
/// };
/// assert_eq!(meta.key, 42);
/// assert_eq!(meta.filename.as_deref(), Some("model.bin"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtentMetadata {
    /// Unique 64-bit key identifying this extent.
    pub key: u64,
    /// Index into the configured set of extent sizes.
    pub size_class: u32,
    /// Actual extent size in bytes (derived from size_class).
    pub extent_size: u32,
    /// NVMe namespace identifier.
    pub ns_id: u32,
    /// Starting offset on the block device in 4KiB blocks.
    pub offset_blocks: u64,
    /// Optional filename (max 256 bytes).
    pub filename: Option<String>,
    /// Optional CRC-32 of the extent data.
    pub data_crc: Option<u32>,
}

impl ExtentMetadata {
    /// Serialize to a compact byte vector for the `IExtentManager` interface.
    ///
    /// Layout: key(8) + size_class(4) + extent_size(4) + ns_id(4) +
    /// offset_blocks(8) + filename_len(2) + filename(N) + has_crc(1) + data_crc(4)
    pub fn to_bytes(&self) -> Vec<u8> {
        let filename_bytes = self.filename.as_deref().unwrap_or("").as_bytes();
        let len = 8 + 4 + 4 + 4 + 8 + 2 + filename_bytes.len() + 1 + 4;
        let mut buf = Vec::with_capacity(len);

        buf.extend_from_slice(&self.key.to_le_bytes());
        buf.extend_from_slice(&self.size_class.to_le_bytes());
        buf.extend_from_slice(&self.extent_size.to_le_bytes());
        buf.extend_from_slice(&self.ns_id.to_le_bytes());
        buf.extend_from_slice(&self.offset_blocks.to_le_bytes());
        buf.extend_from_slice(&(filename_bytes.len() as u16).to_le_bytes());
        buf.extend_from_slice(filename_bytes);
        if let Some(crc) = self.data_crc {
            buf.push(1);
            buf.extend_from_slice(&crc.to_le_bytes());
        } else {
            buf.push(0);
            buf.extend_from_slice(&0u32.to_le_bytes());
        }

        buf
    }

    /// Deserialize from a compact byte vector produced by [`to_bytes`](Self::to_bytes).
    pub fn from_bytes(data: &[u8]) -> Result<Self, ExtentManagerError> {
        if data.len() < 35 {
            return Err(ExtentManagerError::CorruptMetadata(
                "metadata blob too short".into(),
            ));
        }

        let key = u64::from_le_bytes(data[0..8].try_into().unwrap());
        let size_class = u32::from_le_bytes(data[8..12].try_into().unwrap());
        let extent_size = u32::from_le_bytes(data[12..16].try_into().unwrap());
        let ns_id = u32::from_le_bytes(data[16..20].try_into().unwrap());
        let offset_blocks = u64::from_le_bytes(data[20..28].try_into().unwrap());
        let filename_len = u16::from_le_bytes(data[28..30].try_into().unwrap()) as usize;

        if data.len() < 30 + filename_len + 5 {
            return Err(ExtentManagerError::CorruptMetadata(
                "metadata blob truncated".into(),
            ));
        }

        let filename = if filename_len > 0 {
            Some(
                String::from_utf8(data[30..30 + filename_len].to_vec()).map_err(|e| {
                    ExtentManagerError::CorruptMetadata(format!("invalid filename UTF-8: {e}"))
                })?,
            )
        } else {
            None
        };

        let flag_offset = 30 + filename_len;
        let has_crc = data[flag_offset];
        let data_crc = if has_crc == 1 {
            Some(u32::from_le_bytes(
                data[flag_offset + 1..flag_offset + 5].try_into().unwrap(),
            ))
        } else {
            None
        };

        Ok(Self {
            key,
            size_class,
            extent_size,
            ns_id,
            offset_blocks,
            filename,
            data_crc,
        })
    }
}

// ---- On-disk record layout ----
//
// Offsets within a 4096-byte block:
//   0..8     key (u64 LE)
//   8..12    size_class (u32 LE)
//  12..16    ns_id (u32 LE)
//  16..24    offset_blocks (u64 LE)
//  24..26    filename_len (u16 LE)
//  26..282   filename ([u8; 256], zero-padded)
// 282..286   data_crc (u32 LE)
// 286..287   has_crc (u8: 1 = valid, 0 = absent)
// 287..4092  reserved (zero)
// 4092..4096 record_crc (u32 LE over bytes 0..4092)

const OFF_KEY: usize = 0;
const OFF_SIZE_CLASS: usize = 8;
const OFF_NS_ID: usize = 12;
const OFF_OFFSET_BLOCKS: usize = 16;
const OFF_FILENAME_LEN: usize = 24;
const OFF_FILENAME: usize = 26;
const OFF_DATA_CRC: usize = 282;
const OFF_HAS_CRC: usize = 286;
const OFF_RECORD_CRC: usize = 4092;

/// On-disk extent record occupying exactly one 4KiB block.
///
/// # Examples
///
/// ```
/// use extent_manager::metadata::{OnDiskExtentRecord, BLOCK_SIZE};
///
/// let record = OnDiskExtentRecord::new();
/// assert_eq!(record.as_bytes().len(), BLOCK_SIZE);
/// ```
pub struct OnDiskExtentRecord {
    data: [u8; BLOCK_SIZE],
}

impl OnDiskExtentRecord {
    /// Create a new zeroed record.
    pub fn new() -> Self {
        Self {
            data: [0u8; BLOCK_SIZE],
        }
    }

    /// Create a record from a raw 4KiB block.
    pub fn from_bytes(block: [u8; BLOCK_SIZE]) -> Self {
        Self { data: block }
    }

    /// Return the raw bytes of this record.
    pub fn as_bytes(&self) -> &[u8; BLOCK_SIZE] {
        &self.data
    }

    /// Serialize an [`ExtentMetadata`] into this record, computing the CRC.
    ///
    /// # Errors
    ///
    /// Returns [`ExtentManagerError::IoError`] if the filename exceeds
    /// [`MAX_FILENAME_LEN`] bytes.
    pub fn serialize(meta: &ExtentMetadata) -> Result<Self, ExtentManagerError> {
        let mut rec = Self::new();

        rec.data[OFF_KEY..OFF_KEY + 8].copy_from_slice(&meta.key.to_le_bytes());
        rec.data[OFF_SIZE_CLASS..OFF_SIZE_CLASS + 4]
            .copy_from_slice(&meta.size_class.to_le_bytes());
        rec.data[OFF_NS_ID..OFF_NS_ID + 4].copy_from_slice(&meta.ns_id.to_le_bytes());
        rec.data[OFF_OFFSET_BLOCKS..OFF_OFFSET_BLOCKS + 8]
            .copy_from_slice(&meta.offset_blocks.to_le_bytes());

        if let Some(ref name) = meta.filename {
            let name_bytes = name.as_bytes();
            if name_bytes.len() > MAX_FILENAME_LEN {
                return Err(ExtentManagerError::IoError(format!(
                    "filename too long: {} bytes (max {})",
                    name_bytes.len(),
                    MAX_FILENAME_LEN
                )));
            }
            let len = name_bytes.len() as u16;
            rec.data[OFF_FILENAME_LEN..OFF_FILENAME_LEN + 2].copy_from_slice(&len.to_le_bytes());
            rec.data[OFF_FILENAME..OFF_FILENAME + name_bytes.len()].copy_from_slice(name_bytes);
        }

        if let Some(crc) = meta.data_crc {
            rec.data[OFF_DATA_CRC..OFF_DATA_CRC + 4].copy_from_slice(&crc.to_le_bytes());
            rec.data[OFF_HAS_CRC] = 1;
        }

        // Compute and store record CRC over bytes 0..4092.
        let crc = crc32(&rec.data[..OFF_RECORD_CRC]);
        rec.data[OFF_RECORD_CRC..OFF_RECORD_CRC + 4].copy_from_slice(&crc.to_le_bytes());

        Ok(rec)
    }

    /// Deserialize this record into an [`ExtentMetadata`], verifying the CRC.
    ///
    /// # Errors
    ///
    /// Returns [`ExtentManagerError::CorruptMetadata`] if the CRC does not match.
    pub fn deserialize(&self) -> Result<ExtentMetadata, ExtentManagerError> {
        let stored_crc = u32::from_le_bytes(
            self.data[OFF_RECORD_CRC..OFF_RECORD_CRC + 4]
                .try_into()
                .unwrap(),
        );
        let computed_crc = crc32(&self.data[..OFF_RECORD_CRC]);
        if stored_crc != computed_crc {
            return Err(ExtentManagerError::CorruptMetadata(format!(
                "record CRC mismatch: stored={stored_crc:#010x}, computed={computed_crc:#010x}"
            )));
        }

        let key = u64::from_le_bytes(self.data[OFF_KEY..OFF_KEY + 8].try_into().unwrap());
        let size_class = u32::from_le_bytes(
            self.data[OFF_SIZE_CLASS..OFF_SIZE_CLASS + 4]
                .try_into()
                .unwrap(),
        );
        let ns_id = u32::from_le_bytes(self.data[OFF_NS_ID..OFF_NS_ID + 4].try_into().unwrap());
        let offset_blocks = u64::from_le_bytes(
            self.data[OFF_OFFSET_BLOCKS..OFF_OFFSET_BLOCKS + 8]
                .try_into()
                .unwrap(),
        );

        let filename_len = u16::from_le_bytes(
            self.data[OFF_FILENAME_LEN..OFF_FILENAME_LEN + 2]
                .try_into()
                .unwrap(),
        ) as usize;

        let filename = if filename_len > 0 && filename_len <= MAX_FILENAME_LEN {
            Some(
                String::from_utf8(self.data[OFF_FILENAME..OFF_FILENAME + filename_len].to_vec())
                    .map_err(|e| {
                        ExtentManagerError::CorruptMetadata(format!("invalid filename UTF-8: {e}"))
                    })?,
            )
        } else {
            None
        };

        let has_crc = self.data[OFF_HAS_CRC];
        let data_crc = if has_crc == 1 {
            Some(u32::from_le_bytes(
                self.data[OFF_DATA_CRC..OFF_DATA_CRC + 4]
                    .try_into()
                    .unwrap(),
            ))
        } else {
            None
        };

        Ok(ExtentMetadata {
            key,
            size_class,
            extent_size: 0, // Filled by caller from config.
            ns_id,
            offset_blocks,
            filename,
            data_crc,
        })
    }

    /// Check if this record block is all zeros (empty slot).
    pub fn is_empty(&self) -> bool {
        self.data.iter().all(|&b| b == 0)
    }
}

impl Default for OnDiskExtentRecord {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple CRC-32 (IEEE / CRC-32b) implementation.
///
/// Uses the standard polynomial 0xEDB88320 (reflected).
///
/// # Examples
///
/// ```
/// use extent_manager::metadata::crc32;
///
/// let data = b"hello";
/// let c = crc32(data);
/// assert_eq!(c, crc32(data)); // deterministic
/// ```
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_deterministic() {
        let data = b"test data for CRC";
        assert_eq!(crc32(data), crc32(data));
    }

    #[test]
    fn crc32_different_data() {
        assert_ne!(crc32(b"aaa"), crc32(b"bbb"));
    }

    #[test]
    fn extent_metadata_clone_eq() {
        let meta = ExtentMetadata {
            key: 1,
            size_class: 0,
            extent_size: 131072,
            ns_id: 1,
            offset_blocks: 0,
            filename: None,
            data_crc: None,
        };
        assert_eq!(meta, meta.clone());
    }

    #[test]
    fn record_new_is_zeroed() {
        let rec = OnDiskExtentRecord::new();
        assert!(rec.is_empty());
    }

    #[test]
    fn record_round_trip_minimal() {
        let meta = ExtentMetadata {
            key: 42,
            size_class: 0,
            extent_size: 131072,
            ns_id: 1,
            offset_blocks: 100,
            filename: None,
            data_crc: None,
        };
        let rec = OnDiskExtentRecord::serialize(&meta).unwrap();
        let mut restored = rec.deserialize().unwrap();
        restored.extent_size = meta.extent_size;
        assert_eq!(meta, restored);
    }

    #[test]
    fn record_round_trip_with_filename_and_crc() {
        let meta = ExtentMetadata {
            key: 99,
            size_class: 2,
            extent_size: 262144,
            ns_id: 1,
            offset_blocks: 500,
            filename: Some("model.bin".to_string()),
            data_crc: Some(0xDEADBEEF),
        };
        let rec = OnDiskExtentRecord::serialize(&meta).unwrap();
        let mut restored = rec.deserialize().unwrap();
        restored.extent_size = meta.extent_size;
        assert_eq!(meta, restored);
    }

    #[test]
    fn record_detects_corruption() {
        let meta = ExtentMetadata {
            key: 1,
            size_class: 0,
            extent_size: 131072,
            ns_id: 1,
            offset_blocks: 0,
            filename: None,
            data_crc: None,
        };
        let mut rec = OnDiskExtentRecord::serialize(&meta).unwrap();
        // Corrupt a byte.
        rec.data[0] ^= 0xFF;
        assert!(rec.deserialize().is_err());
    }

    #[test]
    fn record_rejects_long_filename() {
        let meta = ExtentMetadata {
            key: 1,
            size_class: 0,
            extent_size: 131072,
            ns_id: 1,
            offset_blocks: 0,
            filename: Some("x".repeat(MAX_FILENAME_LEN + 1)),
            data_crc: None,
        };
        assert!(OnDiskExtentRecord::serialize(&meta).is_err());
    }

    #[test]
    fn record_max_filename_ok() {
        let meta = ExtentMetadata {
            key: 1,
            size_class: 0,
            extent_size: 131072,
            ns_id: 1,
            offset_blocks: 0,
            filename: Some("a".repeat(MAX_FILENAME_LEN)),
            data_crc: None,
        };
        let rec = OnDiskExtentRecord::serialize(&meta).unwrap();
        let mut restored = rec.deserialize().unwrap();
        restored.extent_size = meta.extent_size;
        assert_eq!(meta, restored);
    }

    #[test]
    fn record_from_bytes() {
        let meta = ExtentMetadata {
            key: 7,
            size_class: 1,
            extent_size: 131072,
            ns_id: 1,
            offset_blocks: 50,
            filename: None,
            data_crc: Some(0x12345678),
        };
        let rec = OnDiskExtentRecord::serialize(&meta).unwrap();
        let raw = *rec.as_bytes();
        let rec2 = OnDiskExtentRecord::from_bytes(raw);
        let mut restored = rec2.deserialize().unwrap();
        restored.extent_size = meta.extent_size;
        assert_eq!(meta, restored);
    }

    #[test]
    fn to_bytes_from_bytes_minimal() {
        let meta = ExtentMetadata {
            key: 42,
            size_class: 0,
            extent_size: 131072,
            ns_id: 1,
            offset_blocks: 100,
            filename: None,
            data_crc: None,
        };
        let bytes = meta.to_bytes();
        let restored = ExtentMetadata::from_bytes(&bytes).unwrap();
        assert_eq!(meta, restored);
    }

    #[test]
    fn to_bytes_from_bytes_with_filename_and_crc() {
        let meta = ExtentMetadata {
            key: 99,
            size_class: 2,
            extent_size: 262144,
            ns_id: 1,
            offset_blocks: 500,
            filename: Some("model.bin".to_string()),
            data_crc: Some(0xDEADBEEF),
        };
        let bytes = meta.to_bytes();
        let restored = ExtentMetadata::from_bytes(&bytes).unwrap();
        assert_eq!(meta, restored);
    }

    #[test]
    fn from_bytes_too_short() {
        let short = vec![0u8; 10];
        assert!(ExtentMetadata::from_bytes(&short).is_err());
    }
}
