use crate::metadata::BLOCK_SIZE;
use crc32fast::Hasher;

pub(crate) const SUPERBLOCK_LBA: u64 = 0;
pub(crate) const SUPERBLOCK_MAGIC: u64 = 0x4558_544D_4752_5632; // "EXTMGRV2"
pub(crate) const FORMAT_VERSION: u32 = 2;
pub(crate) const MAX_SLABS: usize = 256;

const SLAB_TABLE_OFFSET: usize = 40;
const SLAB_ENTRY_SIZE: usize = 12; // 4 (size_class) + 8 (start_lba)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SlabEntry {
    pub size_class: u32,
    pub start_lba: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct Superblock {
    pub magic: u64,
    pub version: u32,
    pub namespace_id: u32,
    pub total_blocks: u64,
    pub slab_size_blocks: u32,
    pub num_slabs: u32,
    pub next_free_lba: u64,
    pub slab_table: Vec<SlabEntry>,
}

pub(crate) fn compute_slab_layout(slab_size_blocks: u32) -> (u32, u32) {
    let total = slab_size_blocks as usize;
    if total < 2 {
        return (0, 0);
    }
    let mut num_slots = total - 1;
    loop {
        let bitmap_blocks = num_slots.div_ceil(BITS_PER_BLOCK);
        let available = total - bitmap_blocks;
        if available <= num_slots {
            num_slots = available;
            let bitmap_blocks = num_slots.div_ceil(BITS_PER_BLOCK);
            return (bitmap_blocks as u32, num_slots as u32);
        }
        num_slots = available;
    }
}

const BITS_PER_BLOCK: usize = BLOCK_SIZE * 8;

impl Superblock {
    pub fn new(total_blocks: u64, slab_size_blocks: u32, namespace_id: u32) -> Self {
        Superblock {
            magic: SUPERBLOCK_MAGIC,
            version: FORMAT_VERSION,
            namespace_id,
            total_blocks,
            slab_size_blocks,
            num_slabs: 0,
            next_free_lba: 1, // LBA 0 is the superblock
            slab_table: Vec::new(),
        }
    }

    pub fn add_slab(&mut self, size_class: u32, start_lba: u64) {
        self.slab_table.push(SlabEntry {
            size_class,
            start_lba,
        });
        self.num_slabs = self.slab_table.len() as u32;
        self.next_free_lba = start_lba + self.slab_size_blocks as u64;
    }

    pub fn serialize(&self) -> [u8; BLOCK_SIZE] {
        let mut buf = [0u8; BLOCK_SIZE];
        let mut offset = 0;

        buf[offset..offset + 8].copy_from_slice(&self.magic.to_le_bytes());
        offset += 8;
        buf[offset..offset + 4].copy_from_slice(&self.version.to_le_bytes());
        offset += 4;
        buf[offset..offset + 4].copy_from_slice(&self.namespace_id.to_le_bytes());
        offset += 4;
        buf[offset..offset + 8].copy_from_slice(&self.total_blocks.to_le_bytes());
        offset += 8;
        buf[offset..offset + 4].copy_from_slice(&self.slab_size_blocks.to_le_bytes());
        offset += 4;
        buf[offset..offset + 4].copy_from_slice(&self.num_slabs.to_le_bytes());
        offset += 4;
        buf[offset..offset + 8].copy_from_slice(&self.next_free_lba.to_le_bytes());

        for (i, entry) in self.slab_table.iter().enumerate() {
            let base = SLAB_TABLE_OFFSET + i * SLAB_ENTRY_SIZE;
            buf[base..base + 4].copy_from_slice(&entry.size_class.to_le_bytes());
            buf[base + 4..base + 12].copy_from_slice(&entry.start_lba.to_le_bytes());
        }

        let crc_offset = BLOCK_SIZE - 4;
        let mut hasher = Hasher::new();
        hasher.update(&buf[..crc_offset]);
        let crc = hasher.finalize();
        buf[crc_offset..].copy_from_slice(&crc.to_le_bytes());

        buf
    }

    pub fn deserialize(buf: &[u8; BLOCK_SIZE]) -> Result<Self, String> {
        let crc_offset = BLOCK_SIZE - 4;
        let stored_crc = u32::from_le_bytes(buf[crc_offset..].try_into().unwrap());
        let mut hasher = Hasher::new();
        hasher.update(&buf[..crc_offset]);
        let computed_crc = hasher.finalize();

        if stored_crc != computed_crc {
            return Err(format!(
                "superblock CRC mismatch: stored={stored_crc:#x}, computed={computed_crc:#x}"
            ));
        }

        let magic = u64::from_le_bytes(buf[0..8].try_into().unwrap());
        if magic != SUPERBLOCK_MAGIC {
            return Err(format!("invalid magic: {magic:#x}"));
        }

        let version = u32::from_le_bytes(buf[8..12].try_into().unwrap());
        if version != FORMAT_VERSION {
            return Err(format!("unsupported format version: {version}"));
        }

        let namespace_id = u32::from_le_bytes(buf[12..16].try_into().unwrap());
        let total_blocks = u64::from_le_bytes(buf[16..24].try_into().unwrap());
        let slab_size_blocks = u32::from_le_bytes(buf[24..28].try_into().unwrap());
        let num_slabs = u32::from_le_bytes(buf[28..32].try_into().unwrap());
        let next_free_lba = u64::from_le_bytes(buf[32..40].try_into().unwrap());

        if num_slabs as usize > MAX_SLABS {
            return Err(format!("too many slabs: {num_slabs}"));
        }

        let mut slab_table = Vec::with_capacity(num_slabs as usize);
        for i in 0..num_slabs as usize {
            let base = SLAB_TABLE_OFFSET + i * SLAB_ENTRY_SIZE;
            let size_class = u32::from_le_bytes(buf[base..base + 4].try_into().unwrap());
            let start_lba = u64::from_le_bytes(buf[base + 4..base + 12].try_into().unwrap());
            slab_table.push(SlabEntry {
                size_class,
                start_lba,
            });
        }

        Ok(Superblock {
            magic,
            version,
            namespace_id,
            total_blocks,
            slab_size_blocks,
            num_slabs,
            next_free_lba,
            slab_table,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_deserialize_roundtrip_empty() {
        let sb = Superblock::new(1_000_000, 262144, 1);
        let buf = sb.serialize();
        let restored = Superblock::deserialize(&buf).unwrap();
        assert_eq!(restored.magic, SUPERBLOCK_MAGIC);
        assert_eq!(restored.version, FORMAT_VERSION);
        assert_eq!(restored.total_blocks, 1_000_000);
        assert_eq!(restored.slab_size_blocks, 262144);
        assert_eq!(restored.num_slabs, 0);
        assert_eq!(restored.next_free_lba, 1);
        assert!(restored.slab_table.is_empty());
        assert_eq!(restored.namespace_id, 1);
    }

    #[test]
    fn serialize_deserialize_with_slabs() {
        let mut sb = Superblock::new(1_000_000, 262144, 1);
        sb.add_slab(131072, 1);
        sb.add_slab(524288, 262145);
        let buf = sb.serialize();
        let restored = Superblock::deserialize(&buf).unwrap();
        assert_eq!(restored.num_slabs, 2);
        assert_eq!(restored.next_free_lba, 262145 + 262144);
        assert_eq!(
            restored.slab_table,
            vec![
                SlabEntry {
                    size_class: 131072,
                    start_lba: 1
                },
                SlabEntry {
                    size_class: 524288,
                    start_lba: 262145
                },
            ]
        );
    }

    #[test]
    fn corrupt_crc_detected() {
        let sb = Superblock::new(1_000_000, 262144, 1);
        let mut buf = sb.serialize();
        buf[0] ^= 0xFF;
        assert!(Superblock::deserialize(&buf).is_err());
    }

    #[test]
    fn invalid_magic_rejected() {
        let sb = Superblock::new(1_000_000, 262144, 1);
        let mut buf = sb.serialize();
        buf[0..8].copy_from_slice(&0xBADu64.to_le_bytes());
        let crc_offset = BLOCK_SIZE - 4;
        let mut hasher = Hasher::new();
        hasher.update(&buf[..crc_offset]);
        let crc = hasher.finalize();
        buf[crc_offset..].copy_from_slice(&crc.to_le_bytes());
        assert!(Superblock::deserialize(&buf).is_err());
    }

    #[test]
    fn compute_slab_layout_1gib() {
        let slab_blocks = 262144u32; // 1 GiB / 4 KiB
        let (bitmap_blocks, num_slots) = compute_slab_layout(slab_blocks);
        assert_eq!(bitmap_blocks + num_slots as u32, slab_blocks);
        assert!(num_slots > 0);
        assert!(bitmap_blocks > 0);
        assert!(num_slots.div_ceil(BITS_PER_BLOCK as u32) <= bitmap_blocks);
    }

    #[test]
    fn compute_slab_layout_minimum() {
        let (bitmap_blocks, num_slots) = compute_slab_layout(2);
        assert_eq!(bitmap_blocks, 1);
        assert_eq!(num_slots, 1);
    }

    #[test]
    fn compute_slab_layout_too_small() {
        let (bitmap_blocks, num_slots) = compute_slab_layout(1);
        assert_eq!(bitmap_blocks, 0);
        assert_eq!(num_slots, 0);
    }

    #[test]
    fn max_slabs_boundary() {
        let mut sb = Superblock::new(100_000_000, 1024, 1);
        for i in 0..MAX_SLABS {
            sb.add_slab(131072, 1 + i as u64 * 1024);
        }
        let buf = sb.serialize();
        let restored = Superblock::deserialize(&buf).unwrap();
        assert_eq!(restored.num_slabs, MAX_SLABS as u32);
        assert_eq!(restored.slab_table.len(), MAX_SLABS);
    }
}
