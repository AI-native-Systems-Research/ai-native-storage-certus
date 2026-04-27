use crate::error;
use interfaces::ExtentManagerError;

pub const SUPERBLOCK_SIZE: usize = 4096;
pub const SUPERBLOCK_MAGIC: u64 = 0x4345_5254_5553_5633; // "CERTUSV3"
pub const FORMAT_VERSION: u32 = 3;

#[derive(Debug, Clone)]
pub struct Superblock {
    pub magic: u64,
    pub version: u32,
    pub data_disk_size: u64,
    pub sector_size: u32,
    pub slab_size: u64,
    pub max_extent_size: u32,
    pub region_count: u32,
    pub checkpoint_seq: u64,
    pub active_copy: u8,
    pub checkpoint_region_offset: u64,
    pub checkpoint_region_size: u64,
    pub checksum: u32,
}

impl Superblock {
    pub fn new(
        data_disk_size: u64,
        sector_size: u32,
        slab_size: u64,
        max_extent_size: u32,
        region_count: u32,
        checkpoint_region_offset: u64,
        checkpoint_region_size: u64,
    ) -> Self {
        Self {
            magic: SUPERBLOCK_MAGIC,
            version: FORMAT_VERSION,
            data_disk_size,
            sector_size,
            slab_size,
            max_extent_size,
            region_count,
            checkpoint_seq: 0,
            active_copy: 0,
            checkpoint_region_offset,
            checkpoint_region_size,
            checksum: 0,
        }
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = vec![0u8; SUPERBLOCK_SIZE];
        let mut pos = 0;

        buf[pos..pos + 8].copy_from_slice(&self.magic.to_le_bytes());
        pos += 8;
        buf[pos..pos + 4].copy_from_slice(&self.version.to_le_bytes());
        pos += 4;
        buf[pos..pos + 8].copy_from_slice(&self.data_disk_size.to_le_bytes());
        pos += 8;
        buf[pos..pos + 4].copy_from_slice(&self.sector_size.to_le_bytes());
        pos += 4;
        buf[pos..pos + 8].copy_from_slice(&self.slab_size.to_le_bytes());
        pos += 8;
        buf[pos..pos + 4].copy_from_slice(&self.max_extent_size.to_le_bytes());
        pos += 4;
        buf[pos..pos + 4].copy_from_slice(&self.region_count.to_le_bytes());
        pos += 4;
        buf[pos..pos + 8].copy_from_slice(&self.checkpoint_seq.to_le_bytes());
        pos += 8;
        buf[pos] = self.active_copy;
        pos += 1;
        // 7 bytes reserved
        pos += 7;
        buf[pos..pos + 8].copy_from_slice(&self.checkpoint_region_offset.to_le_bytes());
        pos += 8;
        buf[pos..pos + 8].copy_from_slice(&self.checkpoint_region_size.to_le_bytes());
        pos += 8;

        let crc = crc32fast::hash(&buf[..pos]);
        buf[pos..pos + 4].copy_from_slice(&crc.to_le_bytes());

        buf
    }

    pub fn deserialize(buf: &[u8]) -> Result<Self, ExtentManagerError> {
        if buf.len() < SUPERBLOCK_SIZE {
            return Err(error::corrupt_metadata("superblock too short"));
        }

        let mut pos = 0;

        let magic = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
        pos += 8;
        if magic != SUPERBLOCK_MAGIC {
            return Err(error::corrupt_metadata(&format!(
                "invalid superblock magic: {magic:#x}"
            )));
        }

        let version = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let data_disk_size = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let sector_size = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let slab_size = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let max_extent_size = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let region_count = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let checkpoint_seq = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let active_copy = buf[pos];
        pos += 1;
        // 7 bytes reserved
        pos += 7;
        let checkpoint_region_offset = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let checkpoint_region_size = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
        pos += 8;

        let stored_crc = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
        let computed_crc = crc32fast::hash(&buf[..pos]);

        if stored_crc != computed_crc {
            return Err(error::corrupt_metadata(&format!(
                "superblock CRC mismatch: stored={stored_crc:#x} computed={computed_crc:#x}"
            )));
        }

        Ok(Self {
            magic,
            version,
            data_disk_size,
            sector_size,
            slab_size,
            max_extent_size,
            region_count,
            checkpoint_seq,
            active_copy,
            checkpoint_region_offset,
            checkpoint_region_size,
            checksum: stored_crc,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let sb = Superblock::new(
            1024 * 1024 * 1024,
            4096,
            1024 * 1024,
            65536,
            32,
            1048576 + 4096, // 1 MiB padding + superblock
            512 * 1024 * 1024, // 512 MiB per copy
        );
        let buf = sb.serialize();
        assert_eq!(buf.len(), SUPERBLOCK_SIZE);

        let recovered = Superblock::deserialize(&buf).unwrap();
        assert_eq!(recovered.magic, SUPERBLOCK_MAGIC);
        assert_eq!(recovered.version, FORMAT_VERSION);
        assert_eq!(recovered.data_disk_size, 1024 * 1024 * 1024);
        assert_eq!(recovered.sector_size, 4096);
        assert_eq!(recovered.slab_size, 1024 * 1024);
        assert_eq!(recovered.max_extent_size, 65536);
        assert_eq!(recovered.region_count, 32);
        assert_eq!(recovered.checkpoint_seq, 0);
        assert_eq!(recovered.active_copy, 0);
        assert_eq!(recovered.checkpoint_region_offset, 1048576 + 4096);
        assert_eq!(recovered.checkpoint_region_size, 512 * 1024 * 1024);
    }

    #[test]
    fn corrupt_crc_detected() {
        let sb = Superblock::new(1024 * 1024, 4096, 65536, 4096, 32, 4096 + 1048576, 65536);
        let mut buf = sb.serialize();
        buf[10] ^= 0xFF;
        let err = Superblock::deserialize(&buf).unwrap_err();
        assert!(err.to_string().contains("CRC mismatch"));
    }

    #[test]
    fn invalid_magic_detected() {
        let sb = Superblock::new(1024 * 1024, 4096, 65536, 4096, 32, 4096 + 1048576, 65536);
        let mut buf = sb.serialize();
        buf[0] = 0xFF;
        let err = Superblock::deserialize(&buf).unwrap_err();
        assert!(err.to_string().contains("magic"));
    }
}
