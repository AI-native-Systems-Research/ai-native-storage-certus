use crc32fast::Hasher;

pub(crate) const BLOCK_SIZE: usize = 4096;
pub(crate) const MAX_FILENAME_LEN: usize = 255;
pub(crate) const RECORD_CRC_OFFSET: usize = BLOCK_SIZE - 4;

#[derive(Debug, Clone)]
pub(crate) struct ExtentMetadata {
    pub key: u64,
    pub size_class: u32,
    pub namespace_id: u32,
    pub offset_lba: u64,
    pub filename: Option<String>,
    pub data_crc: Option<u32>,
    pub slab_index: usize,
}

impl ExtentMetadata {
    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = vec![0u8; 64];
        buf[0..8].copy_from_slice(&self.key.to_le_bytes());
        buf[8..12].copy_from_slice(&self.size_class.to_le_bytes());
        buf[12..16].copy_from_slice(&self.namespace_id.to_le_bytes());
        buf[16..24].copy_from_slice(&self.offset_lba.to_le_bytes());
        if let Some(ref fname) = self.filename {
            let bytes = fname.as_bytes();
            let len = bytes.len().min(MAX_FILENAME_LEN) as u16;
            buf[24..26].copy_from_slice(&len.to_le_bytes());
            buf.extend_from_slice(&bytes[..len as usize]);
        }
        if let Some(crc) = self.data_crc {
            buf[26..30].copy_from_slice(&crc.to_le_bytes());
            buf[30] = 1;
        }
        buf
    }

    #[allow(dead_code)]
    pub fn deserialize(data: &[u8]) -> Option<Self> {
        if data.len() < 31 {
            return None;
        }
        let key = u64::from_le_bytes(data[0..8].try_into().ok()?);
        if key == 0 {
            return None;
        }
        let size_class = u32::from_le_bytes(data[8..12].try_into().ok()?);
        let namespace_id = u32::from_le_bytes(data[12..16].try_into().ok()?);
        let offset_lba = u64::from_le_bytes(data[16..24].try_into().ok()?);
        let filename_len = u16::from_le_bytes(data[24..26].try_into().ok()?) as usize;
        let data_crc_val = u32::from_le_bytes(data[26..30].try_into().ok()?);
        let has_crc = data[30] != 0;

        let filename = if filename_len > 0 && data.len() >= 31 + filename_len {
            Some(String::from_utf8_lossy(&data[31..31 + filename_len]).into_owned())
        } else {
            None
        };

        let data_crc = if has_crc { Some(data_crc_val) } else { None };

        Some(ExtentMetadata {
            key,
            size_class,
            namespace_id,
            offset_lba,
            filename,
            data_crc,
            slab_index: 0,
        })
    }
}

#[derive(Debug)]
pub(crate) struct OnDiskExtentRecord {
    pub data: [u8; BLOCK_SIZE],
}

impl OnDiskExtentRecord {
    pub fn from_metadata(meta: &ExtentMetadata) -> Self {
        let mut data = [0u8; BLOCK_SIZE];

        data[0..8].copy_from_slice(&meta.key.to_le_bytes());
        data[8..12].copy_from_slice(&meta.size_class.to_le_bytes());
        data[12..16].copy_from_slice(&meta.namespace_id.to_le_bytes());
        data[16..24].copy_from_slice(&meta.offset_lba.to_le_bytes());

        let has_crc: u8 = if meta.data_crc.is_some() { 1 } else { 0 };
        data[24] = has_crc;
        if let Some(crc) = meta.data_crc {
            data[25..29].copy_from_slice(&crc.to_le_bytes());
        }

        if let Some(ref fname) = meta.filename {
            let bytes = fname.as_bytes();
            let len = bytes.len().min(MAX_FILENAME_LEN);
            data[29..31].copy_from_slice(&(len as u16).to_le_bytes());
            data[31..31 + len].copy_from_slice(&bytes[..len]);
        }

        let crc = compute_record_crc(&data);
        data[RECORD_CRC_OFFSET..BLOCK_SIZE].copy_from_slice(&crc.to_le_bytes());

        OnDiskExtentRecord { data }
    }

    pub fn to_metadata(&self) -> Option<ExtentMetadata> {
        let key = u64::from_le_bytes(self.data[0..8].try_into().ok()?);
        if key == 0 {
            return None;
        }

        let size_class = u32::from_le_bytes(self.data[8..12].try_into().ok()?);
        let namespace_id = u32::from_le_bytes(self.data[12..16].try_into().ok()?);
        let offset_lba = u64::from_le_bytes(self.data[16..24].try_into().ok()?);
        let has_crc = self.data[24] != 0;
        let data_crc_val = u32::from_le_bytes(self.data[25..29].try_into().ok()?);
        let filename_len = u16::from_le_bytes(self.data[29..31].try_into().ok()?) as usize;

        let filename = if filename_len > 0 && filename_len <= MAX_FILENAME_LEN {
            Some(String::from_utf8_lossy(&self.data[31..31 + filename_len]).into_owned())
        } else {
            None
        };

        let data_crc = if has_crc { Some(data_crc_val) } else { None };

        Some(ExtentMetadata {
            key,
            size_class,
            namespace_id,
            offset_lba,
            filename,
            data_crc,
            slab_index: 0,
        })
    }

    pub fn verify_crc(&self) -> bool {
        let stored = u32::from_le_bytes(
            self.data[RECORD_CRC_OFFSET..BLOCK_SIZE]
                .try_into()
                .unwrap_or([0; 4]),
        );
        let computed = compute_record_crc(&self.data);
        stored == computed
    }

    pub fn is_empty(&self) -> bool {
        u64::from_le_bytes(self.data[0..8].try_into().unwrap_or([0; 8])) == 0
    }
}

pub(crate) fn compute_record_crc(data: &[u8; BLOCK_SIZE]) -> u32 {
    let mut hasher = Hasher::new();
    hasher.update(&data[..RECORD_CRC_OFFSET]);
    hasher.finalize()
}

pub(crate) fn zero_block() -> [u8; BLOCK_SIZE] {
    [0u8; BLOCK_SIZE]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_roundtrip_no_optional_fields() {
        let meta = ExtentMetadata {
            key: 42,
            size_class: 131072,
            namespace_id: 1,
            offset_lba: 100,
            filename: None,
            data_crc: None,
            slab_index: 0,
        };
        let record = OnDiskExtentRecord::from_metadata(&meta);
        assert!(record.verify_crc());
        let recovered = record.to_metadata().unwrap();
        assert_eq!(recovered.key, 42);
        assert_eq!(recovered.size_class, 131072);
        assert_eq!(recovered.offset_lba, 100);
        assert!(recovered.filename.is_none());
        assert!(recovered.data_crc.is_none());
    }

    #[test]
    fn record_roundtrip_with_filename_and_crc() {
        let meta = ExtentMetadata {
            key: 99,
            size_class: 524288,
            namespace_id: 2,
            offset_lba: 500,
            filename: Some("test-file.dat".to_string()),
            data_crc: Some(0xDEADBEEF),
            slab_index: 0,
        };
        let record = OnDiskExtentRecord::from_metadata(&meta);
        assert!(record.verify_crc());
        let recovered = record.to_metadata().unwrap();
        assert_eq!(recovered.key, 99);
        assert_eq!(recovered.filename.as_deref(), Some("test-file.dat"));
        assert_eq!(recovered.data_crc, Some(0xDEADBEEF));
    }

    #[test]
    fn empty_record_returns_none() {
        let record = OnDiskExtentRecord {
            data: [0u8; BLOCK_SIZE],
        };
        assert!(record.is_empty());
        assert!(record.to_metadata().is_none());
    }

    #[test]
    fn corrupt_crc_detected() {
        let meta = ExtentMetadata {
            key: 1,
            size_class: 131072,
            namespace_id: 1,
            offset_lba: 0,
            filename: None,
            data_crc: None,
            slab_index: 0,
        };
        let mut record = OnDiskExtentRecord::from_metadata(&meta);
        record.data[0] ^= 0xFF;
        assert!(!record.verify_crc());
    }

    #[test]
    fn max_filename_length() {
        let long_name = "a".repeat(MAX_FILENAME_LEN);
        let meta = ExtentMetadata {
            key: 7,
            size_class: 131072,
            namespace_id: 1,
            offset_lba: 10,
            filename: Some(long_name.clone()),
            data_crc: None,
            slab_index: 0,
        };
        let record = OnDiskExtentRecord::from_metadata(&meta);
        assert!(record.verify_crc());
        let recovered = record.to_metadata().unwrap();
        assert_eq!(recovered.filename.as_deref(), Some(long_name.as_str()));
    }

    #[test]
    fn metadata_serialize_deserialize() {
        let meta = ExtentMetadata {
            key: 123,
            size_class: 262144,
            namespace_id: 3,
            offset_lba: 999,
            filename: Some("hello.bin".to_string()),
            data_crc: Some(0x12345678),
            slab_index: 0,
        };
        let serialized = meta.serialize();
        let recovered = ExtentMetadata::deserialize(&serialized).unwrap();
        assert_eq!(recovered.key, 123);
        assert_eq!(recovered.size_class, 262144);
        assert_eq!(recovered.namespace_id, 3);
        assert_eq!(recovered.offset_lba, 999);
    }
}
