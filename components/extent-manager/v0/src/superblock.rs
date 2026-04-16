//! Superblock layout, validation, and persistence.

use crate::error::ExtentManagerError;
use crate::metadata::{crc32, BLOCK_SIZE};

/// Magic number identifying a valid extent-manager superblock.
pub const SUPERBLOCK_MAGIC: u64 = 0x4558_544D_4752_5631; // "EXTMGRV1"

/// Current on-disk format version.
pub const FORMAT_VERSION: u32 = 1;

/// Maximum number of supported size classes.
pub const MAX_SIZE_CLASSES: usize = 32;

/// Superblock LBA (always block 0).
pub const SUPERBLOCK_LBA: u64 = 0;

// On-disk layout within a 4096-byte block:
//   0..8     magic (u64 LE)
//   8..12    version (u32 LE)
//  12..16    num_size_classes (u32 LE)
//  16..144   size_table ([u32; 32] LE, 128 bytes)
// 144..272   slots_per_class ([u32; 32] LE, 128 bytes)
// 272..280   bitmap_start_block (u64 LE)
// 280..288   records_start_block (u64 LE)
// 288..4092  reserved (zero)
// 4092..4096 checksum (u32 LE over bytes 0..4092)

const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 8;
const OFF_NUM_CLASSES: usize = 12;
const OFF_SIZE_TABLE: usize = 16;
const OFF_SLOTS_TABLE: usize = 144;
const OFF_BITMAP_START: usize = 272;
const OFF_RECORDS_START: usize = 280;
const OFF_CHECKSUM: usize = 4092;

/// In-memory representation of the superblock.
///
/// # Examples
///
/// ```ignore
/// use extent_manager::superblock::Superblock;
///
/// let sb = Superblock::new(
///     &[131072, 262144],   // 128KiB, 256KiB
///     &[1000, 500],        // slots per class
/// ).unwrap();
/// assert_eq!(sb.num_size_classes(), 2);
/// assert_eq!(sb.size_for_class(0), Some(131072));
/// ```
#[derive(Debug, Clone)]
pub struct Superblock {
    num_size_classes: u32,
    size_table: [u32; MAX_SIZE_CLASSES],
    slots_per_class: [u32; MAX_SIZE_CLASSES],
    bitmap_start_block: u64,
    records_start_block: u64,
}

impl Superblock {
    /// Create a new superblock from size classes and slot counts.
    ///
    /// Computes the bitmap and record region offsets automatically.
    ///
    /// # Errors
    ///
    /// Returns an error if no size classes are provided or more than 32
    /// are given.
    pub(crate) fn new(sizes: &[u32], slots: &[u32]) -> Result<Self, ExtentManagerError> {
        if sizes.is_empty() || sizes.len() > MAX_SIZE_CLASSES {
            return Err(ExtentManagerError::IoError(format!(
                "size classes must be 1..={MAX_SIZE_CLASSES}, got {}",
                sizes.len()
            )));
        }
        if sizes.len() != slots.len() {
            return Err(ExtentManagerError::IoError(
                "sizes and slots arrays must have equal length".into(),
            ));
        }

        let mut size_table = [0u32; MAX_SIZE_CLASSES];
        let mut slots_per_class = [0u32; MAX_SIZE_CLASSES];
        for (i, (&s, &sl)) in sizes.iter().zip(slots.iter()).enumerate() {
            size_table[i] = s;
            slots_per_class[i] = sl;
        }

        let num_classes = sizes.len() as u32;

        // Bitmap region starts at block 1 (after superblock).
        let bitmap_start_block = 1u64;

        // Compute total bitmap blocks across all size classes.
        let mut total_bitmap_blocks = 0u64;
        let bits_per_block = (BLOCK_SIZE * 8) as u64;
        for &slot_count in slots_per_class.iter().take(num_classes as usize) {
            let bits = slot_count as u64;
            total_bitmap_blocks += bits.div_ceil(bits_per_block);
        }

        let records_start_block = bitmap_start_block + total_bitmap_blocks;

        Ok(Self {
            num_size_classes: num_classes,
            size_table,
            slots_per_class,
            bitmap_start_block,
            records_start_block,
        })
    }

    /// Number of configured size classes.
    pub(crate) fn num_size_classes(&self) -> u32 {
        self.num_size_classes
    }

    /// Extent size in bytes for a given size class index.
    pub(crate) fn size_for_class(&self, class: u32) -> Option<u32> {
        if class < self.num_size_classes {
            Some(self.size_table[class as usize])
        } else {
            None
        }
    }

    /// Maximum slot count for a given size class.
    pub(crate) fn slots_for_class(&self, class: u32) -> Option<u32> {
        if class < self.num_size_classes {
            Some(self.slots_per_class[class as usize])
        } else {
            None
        }
    }

    /// First block of the bitmap region.
    pub(crate) fn bitmap_start_block(&self) -> u64 {
        self.bitmap_start_block
    }

    /// First block of the extent record region.
    pub(crate) fn records_start_block(&self) -> u64 {
        self.records_start_block
    }

    /// Compute the starting LBA of the bitmap for a specific size class.
    pub(crate) fn bitmap_lba_for_class(&self, class: u32) -> Option<u64> {
        if class >= self.num_size_classes {
            return None;
        }
        let mut lba = self.bitmap_start_block;
        let bits_per_block = (BLOCK_SIZE * 8) as u64;
        for &slot_count in self.slots_per_class.iter().take(class as usize) {
            let bits = slot_count as u64;
            lba += bits.div_ceil(bits_per_block);
        }
        Some(lba)
    }

    /// Number of bitmap blocks for a given size class.
    pub fn bitmap_blocks_for_class(&self, class: u32) -> Option<u64> {
        if class >= self.num_size_classes {
            return None;
        }
        let bits = self.slots_per_class[class as usize] as u64;
        let bits_per_block = (BLOCK_SIZE * 8) as u64;
        Some(bits.div_ceil(bits_per_block))
    }

    /// Compute the LBA of an extent record given a global slot index.
    pub fn record_lba(&self, global_slot: u64) -> u64 {
        self.records_start_block + global_slot
    }

    /// Convert (size_class, slot_within_class) to a global slot index.
    pub fn global_slot(&self, class: u32, slot: u32) -> u64 {
        let mut offset = 0u64;
        for i in 0..class as usize {
            offset += self.slots_per_class[i] as u64;
        }
        offset + slot as u64
    }

    /// Total number of blocks needed on the device (superblock + bitmaps + records).
    pub fn total_blocks_required(&self) -> u64 {
        let total_slots: u64 = self
            .slots_per_class
            .iter()
            .take(self.num_size_classes as usize)
            .map(|&s| s as u64)
            .sum();
        self.records_start_block + total_slots
    }

    /// Serialize to a 4KiB block.
    pub fn serialize(&self) -> [u8; BLOCK_SIZE] {
        let mut block = [0u8; BLOCK_SIZE];

        block[OFF_MAGIC..OFF_MAGIC + 8].copy_from_slice(&SUPERBLOCK_MAGIC.to_le_bytes());
        block[OFF_VERSION..OFF_VERSION + 4].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        block[OFF_NUM_CLASSES..OFF_NUM_CLASSES + 4]
            .copy_from_slice(&self.num_size_classes.to_le_bytes());

        for (i, &size) in self.size_table.iter().enumerate() {
            let off = OFF_SIZE_TABLE + i * 4;
            block[off..off + 4].copy_from_slice(&size.to_le_bytes());
        }
        for (i, &slots) in self.slots_per_class.iter().enumerate() {
            let off = OFF_SLOTS_TABLE + i * 4;
            block[off..off + 4].copy_from_slice(&slots.to_le_bytes());
        }

        block[OFF_BITMAP_START..OFF_BITMAP_START + 8]
            .copy_from_slice(&self.bitmap_start_block.to_le_bytes());
        block[OFF_RECORDS_START..OFF_RECORDS_START + 8]
            .copy_from_slice(&self.records_start_block.to_le_bytes());

        let checksum = crc32(&block[..OFF_CHECKSUM]);
        block[OFF_CHECKSUM..OFF_CHECKSUM + 4].copy_from_slice(&checksum.to_le_bytes());

        block
    }

    /// Deserialize from a 4KiB block, validating magic, version, and checksum.
    ///
    /// # Errors
    ///
    /// Returns [`ExtentManagerError::CorruptMetadata`] if validation fails.
    pub fn deserialize(block: &[u8; BLOCK_SIZE]) -> Result<Self, ExtentManagerError> {
        let magic = u64::from_le_bytes(block[OFF_MAGIC..OFF_MAGIC + 8].try_into().unwrap());
        if magic != SUPERBLOCK_MAGIC {
            return Err(ExtentManagerError::CorruptMetadata(format!(
                "bad superblock magic: {magic:#018x}"
            )));
        }

        let version = u32::from_le_bytes(block[OFF_VERSION..OFF_VERSION + 4].try_into().unwrap());
        if version != FORMAT_VERSION {
            return Err(ExtentManagerError::CorruptMetadata(format!(
                "unsupported format version: {version}"
            )));
        }

        let stored_checksum =
            u32::from_le_bytes(block[OFF_CHECKSUM..OFF_CHECKSUM + 4].try_into().unwrap());
        let computed = crc32(&block[..OFF_CHECKSUM]);
        if stored_checksum != computed {
            return Err(ExtentManagerError::CorruptMetadata(
                "superblock checksum mismatch".into(),
            ));
        }

        let num_size_classes = u32::from_le_bytes(
            block[OFF_NUM_CLASSES..OFF_NUM_CLASSES + 4]
                .try_into()
                .unwrap(),
        );

        let mut size_table = [0u32; MAX_SIZE_CLASSES];
        for (i, entry) in size_table.iter_mut().enumerate() {
            let off = OFF_SIZE_TABLE + i * 4;
            *entry = u32::from_le_bytes(block[off..off + 4].try_into().unwrap());
        }

        let mut slots_per_class = [0u32; MAX_SIZE_CLASSES];
        for (i, entry) in slots_per_class.iter_mut().enumerate() {
            let off = OFF_SLOTS_TABLE + i * 4;
            *entry = u32::from_le_bytes(block[off..off + 4].try_into().unwrap());
        }

        let bitmap_start_block = u64::from_le_bytes(
            block[OFF_BITMAP_START..OFF_BITMAP_START + 8]
                .try_into()
                .unwrap(),
        );
        let records_start_block = u64::from_le_bytes(
            block[OFF_RECORDS_START..OFF_RECORDS_START + 8]
                .try_into()
                .unwrap(),
        );

        Ok(Self {
            num_size_classes,
            size_table,
            slots_per_class,
            bitmap_start_block,
            records_start_block,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn superblock_round_trip() {
        let sb = Superblock::new(&[131072, 262144], &[1000, 500]).unwrap();
        let block = sb.serialize();
        let restored = Superblock::deserialize(&block).unwrap();
        assert_eq!(restored.num_size_classes(), 2);
        assert_eq!(restored.size_for_class(0), Some(131072));
        assert_eq!(restored.size_for_class(1), Some(262144));
        assert_eq!(restored.slots_for_class(0), Some(1000));
        assert_eq!(restored.slots_for_class(1), Some(500));
    }

    #[test]
    fn superblock_bad_magic() {
        let mut block = [0u8; BLOCK_SIZE];
        block[0..8].copy_from_slice(&0xBADu64.to_le_bytes());
        assert!(Superblock::deserialize(&block).is_err());
    }

    #[test]
    fn superblock_bad_checksum() {
        let sb = Superblock::new(&[131072], &[100]).unwrap();
        let mut block = sb.serialize();
        block[100] ^= 0xFF; // corrupt
        assert!(Superblock::deserialize(&block).is_err());
    }

    #[test]
    fn superblock_no_size_classes() {
        assert!(Superblock::new(&[], &[]).is_err());
    }

    #[test]
    fn superblock_too_many_classes() {
        let sizes: Vec<u32> = (0..33).map(|i| (i + 1) * 4096).collect();
        let slots: Vec<u32> = vec![100; 33];
        assert!(Superblock::new(&sizes, &slots).is_err());
    }

    #[test]
    fn superblock_mismatched_lengths() {
        assert!(Superblock::new(&[131072, 262144], &[100]).is_err());
    }

    #[test]
    fn bitmap_lba_calculation() {
        let sb = Superblock::new(&[131072, 262144], &[1000, 500]).unwrap();
        // Class 0: 1000 bits = 1 block (32768 bits per block)
        assert_eq!(sb.bitmap_lba_for_class(0), Some(1));
        assert_eq!(sb.bitmap_lba_for_class(1), Some(2));
        assert_eq!(sb.bitmap_lba_for_class(2), None);
    }

    #[test]
    fn records_start_after_bitmaps() {
        let sb = Superblock::new(&[131072], &[100_000]).unwrap();
        // 100K bits = ceil(100000/32768) = 4 bitmap blocks
        assert_eq!(sb.bitmap_start_block(), 1);
        assert_eq!(sb.records_start_block(), 5); // 1 + 4
    }

    #[test]
    fn global_slot_calculation() {
        let sb = Superblock::new(&[131072, 262144], &[1000, 500]).unwrap();
        assert_eq!(sb.global_slot(0, 0), 0);
        assert_eq!(sb.global_slot(0, 999), 999);
        assert_eq!(sb.global_slot(1, 0), 1000);
        assert_eq!(sb.global_slot(1, 499), 1499);
    }

    #[test]
    fn total_blocks_required() {
        let sb = Superblock::new(&[131072], &[100]).unwrap();
        // superblock(1) + bitmap(1) + records(100) = 102
        assert_eq!(sb.total_blocks_required(), 102);
    }
}
