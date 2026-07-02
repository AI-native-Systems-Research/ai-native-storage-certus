//! On-disk format definitions for the extended metadata store.
//!
//! Layout: Superblock (1 sector) | Region A | Region B
//! Each region contains: RegionHeader (1 sector) | Entry Records (variable, sector-aligned)

const SUPERBLOCK_MAGIC: u64 = 0x4345_5254_4D45_5441; // "CERTMETA"
const FORMAT_VERSION: u32 = 1;

/// Superblock occupies exactly one sector (4096 bytes by default).
#[derive(Debug, Clone)]
pub struct Superblock {
    pub magic: u64,
    pub version: u32,
    pub sector_size: u32,
    pub partition_sectors: u64,
    pub region_a_offset: u64,
    pub region_a_size: u64,
    pub region_b_offset: u64,
    pub region_b_size: u64,
    pub active_region: u64,
    pub flush_seq: u64,
    pub entry_count: u64,
    pub crc32: u32,
}

impl Superblock {
    pub fn new(sector_size: u32, partition_sectors: u64) -> Self {
        let superblock_sectors = 1u64;
        let (_usable_sectors, region_size, region_a_offset, region_b_offset) =
            if partition_sectors > superblock_sectors {
                let usable = partition_sectors - superblock_sectors;
                let region = usable / 2;
                (usable, region, superblock_sectors, superblock_sectors + region)
            } else {
                (0, 0, 0, 0)
            };

        Self {
            magic: SUPERBLOCK_MAGIC,
            version: FORMAT_VERSION,
            sector_size,
            partition_sectors,
            region_a_offset,
            region_a_size: region_size,
            region_b_offset,
            region_b_size: region_size,
            active_region: 0,
            flush_seq: 0,
            entry_count: 0,
            crc32: 0,
        }
    }

    pub fn serialize(&self, sector_size: usize) -> Vec<u8> {
        let mut buf = vec![0u8; sector_size];
        let mut pos = 0;

        write_u64(&mut buf, &mut pos, self.magic);
        write_u32(&mut buf, &mut pos, self.version);
        write_u32(&mut buf, &mut pos, self.sector_size);
        write_u64(&mut buf, &mut pos, self.partition_sectors);
        write_u64(&mut buf, &mut pos, self.region_a_offset);
        write_u64(&mut buf, &mut pos, self.region_a_size);
        write_u64(&mut buf, &mut pos, self.region_b_offset);
        write_u64(&mut buf, &mut pos, self.region_b_size);
        write_u64(&mut buf, &mut pos, self.active_region);
        write_u64(&mut buf, &mut pos, self.flush_seq);
        write_u64(&mut buf, &mut pos, self.entry_count);
        // CRC position
        let crc_pos = pos;
        write_u32(&mut buf, &mut pos, 0); // placeholder

        let crc = crc32_of(&buf[..crc_pos]);
        write_u32(&mut buf, &mut crc_pos.clone(), crc);
        buf[crc_pos..crc_pos + 4].copy_from_slice(&crc.to_le_bytes());

        buf
    }

    pub fn deserialize(data: &[u8]) -> Option<Self> {
        if data.len() < 84 {
            return None;
        }
        let mut pos = 0;

        let magic = read_u64(data, &mut pos);
        if magic != SUPERBLOCK_MAGIC {
            return None;
        }
        let version = read_u32(data, &mut pos);
        if version != FORMAT_VERSION {
            return None;
        }
        let sector_size = read_u32(data, &mut pos);
        let partition_sectors = read_u64(data, &mut pos);
        let region_a_offset = read_u64(data, &mut pos);
        let region_a_size = read_u64(data, &mut pos);
        let region_b_offset = read_u64(data, &mut pos);
        let region_b_size = read_u64(data, &mut pos);
        let active_region = read_u64(data, &mut pos);
        let flush_seq = read_u64(data, &mut pos);
        let entry_count = read_u64(data, &mut pos);
        let stored_crc = read_u32(data, &mut pos);

        let computed_crc = crc32_of(&data[..pos - 4]);
        if computed_crc != stored_crc {
            return None;
        }

        Some(Self {
            magic,
            version,
            sector_size,
            partition_sectors,
            region_a_offset,
            region_a_size,
            region_b_offset,
            region_b_size,
            active_region,
            flush_seq,
            entry_count,
            crc32: stored_crc,
        })
    }

    pub fn inactive_region_offset(&self) -> u64 {
        if self.active_region == 0 {
            self.region_b_offset
        } else {
            self.region_a_offset
        }
    }

    pub fn active_region_offset(&self) -> u64 {
        if self.active_region == 0 {
            self.region_a_offset
        } else {
            self.region_b_offset
        }
    }

    pub fn region_capacity_bytes(&self) -> u64 {
        self.region_a_size * self.sector_size as u64
    }
}

/// Header at the start of each region (1 sector).
#[derive(Debug, Clone)]
pub struct RegionHeader {
    pub flush_seq: u64,
    pub entry_count: u32,
    pub total_data_bytes: u32,
    pub crc32: u32,
}

impl RegionHeader {
    pub fn serialize(&self, sector_size: usize) -> Vec<u8> {
        let mut buf = vec![0u8; sector_size];
        let mut pos = 0;

        write_u64(&mut buf, &mut pos, self.flush_seq);
        write_u32(&mut buf, &mut pos, self.entry_count);
        write_u32(&mut buf, &mut pos, self.total_data_bytes);

        let crc = crc32_of(&buf[..pos]);
        write_u32(&mut buf, &mut pos, crc);

        buf
    }

    pub fn deserialize(data: &[u8]) -> Option<Self> {
        if data.len() < 20 {
            return None;
        }
        let mut pos = 0;
        let flush_seq = read_u64(data, &mut pos);
        let entry_count = read_u32(data, &mut pos);
        let total_data_bytes = read_u32(data, &mut pos);
        let stored_crc = read_u32(data, &mut pos);

        let computed_crc = crc32_of(&data[..pos - 4]);
        if computed_crc != stored_crc {
            return None;
        }

        Some(Self {
            flush_seq,
            entry_count,
            total_data_bytes,
            crc32: stored_crc,
        })
    }
}

/// A single key-value entry record on disk.
///
/// Format: [key_len: 2B][value_len: 4B][flags: 2B][crc32: 4B][key bytes][value bytes]
/// Padded to sector alignment.
#[derive(Debug, Clone)]
pub struct EntryRecord {
    pub key: String,
    pub value: Vec<u8>,
    pub flags: u16,
}

impl EntryRecord {
    const HEADER_SIZE: usize = 2 + 4 + 2 + 4; // key_len + value_len + flags + crc32 = 12

    pub fn new(key: String, value: Vec<u8>) -> Self {
        Self { key, value, flags: 0 }
    }

    pub fn serialized_size(&self, sector_size: usize) -> usize {
        let raw = Self::HEADER_SIZE + self.key.len() + self.value.len();
        pad_to_sector(raw, sector_size)
    }

    pub fn serialize(&self, sector_size: usize) -> Vec<u8> {
        let total_size = self.serialized_size(sector_size);
        let mut buf = vec![0u8; total_size];
        let mut pos = 0;

        let key_bytes = self.key.as_bytes();
        write_u16(&mut buf, &mut pos, key_bytes.len() as u16);
        write_u32(&mut buf, &mut pos, self.value.len() as u32);
        write_u16(&mut buf, &mut pos, self.flags);

        // CRC placeholder position
        let crc_pos = pos;
        write_u32(&mut buf, &mut pos, 0);

        // Key + value
        buf[pos..pos + key_bytes.len()].copy_from_slice(key_bytes);
        pos += key_bytes.len();
        buf[pos..pos + self.value.len()].copy_from_slice(&self.value);
        pos += self.value.len();

        // Compute CRC over everything except the CRC field itself
        let mut crc_data = Vec::with_capacity(pos - 4);
        crc_data.extend_from_slice(&buf[..crc_pos]);
        crc_data.extend_from_slice(&buf[crc_pos + 4..pos]);
        let crc = crc32_of(&crc_data);
        buf[crc_pos..crc_pos + 4].copy_from_slice(&crc.to_le_bytes());

        buf
    }

    pub fn deserialize(data: &[u8]) -> Option<(Self, usize)> {
        if data.len() < Self::HEADER_SIZE {
            return None;
        }
        let mut pos = 0;
        let key_len = read_u16(data, &mut pos) as usize;
        let value_len = read_u32(data, &mut pos) as usize;
        let flags = read_u16(data, &mut pos);
        let stored_crc = read_u32(data, &mut pos);

        let needed = Self::HEADER_SIZE + key_len + value_len;
        if data.len() < needed {
            return None;
        }

        let key_bytes = &data[pos..pos + key_len];
        let value_bytes = &data[pos + key_len..pos + key_len + value_len];

        // Verify CRC
        let mut crc_data = Vec::with_capacity(needed - 4);
        crc_data.extend_from_slice(&data[..Self::HEADER_SIZE - 4]); // header sans CRC
        crc_data.extend_from_slice(&data[Self::HEADER_SIZE..Self::HEADER_SIZE + key_len + value_len]);
        let computed_crc = crc32_of(&crc_data);
        if computed_crc != stored_crc {
            return None;
        }

        let key = String::from_utf8(key_bytes.to_vec()).ok()?;
        let value = value_bytes.to_vec();

        Some((Self { key, value, flags }, needed))
    }
}

/// Serialize all entries into a region blob (header + entries), sector-aligned.
pub fn serialize_region(
    entries: &[(String, Vec<u8>)],
    flush_seq: u64,
    sector_size: usize,
) -> Vec<u8> {
    let mut entry_data = Vec::new();
    for (key, value) in entries {
        let record = EntryRecord::new(key.clone(), value.clone());
        entry_data.extend_from_slice(&record.serialize(sector_size));
    }

    let header = RegionHeader {
        flush_seq,
        entry_count: entries.len() as u32,
        total_data_bytes: entry_data.len() as u32,
        crc32: 0,
    };
    let header_bytes = header.serialize(sector_size);

    let mut result = header_bytes;
    result.extend_from_slice(&entry_data);
    result
}

/// Key-value entry pair type.
pub type KvEntry = (String, Vec<u8>);

/// Deserialize a region blob into key-value pairs. Skips corrupt entries.
#[allow(clippy::type_complexity)]
pub fn deserialize_region(
    data: &[u8],
    sector_size: usize,
) -> Option<(RegionHeader, Vec<KvEntry>)> {
    if data.len() < sector_size {
        return None;
    }

    let header = RegionHeader::deserialize(&data[..sector_size])?;
    let mut entries = Vec::new();
    let mut offset = sector_size;

    for _ in 0..header.entry_count {
        if offset >= data.len() {
            break;
        }
        match EntryRecord::deserialize(&data[offset..]) {
            Some((record, raw_size)) => {
                let padded = pad_to_sector(raw_size, sector_size);
                entries.push((record.key, record.value));
                offset += padded;
            }
            None => {
                // Corrupt entry — skip to next sector boundary
                offset += sector_size;
            }
        }
    }

    Some((header, entries))
}

// --- Helpers ---

pub fn pad_to_sector(size: usize, sector_size: usize) -> usize {
    if size == 0 {
        return 0;
    }
    size.div_ceil(sector_size) * sector_size
}

pub fn bytes_to_sectors(bytes: usize, sector_size: usize) -> u64 {
    pad_to_sector(bytes, sector_size) as u64 / sector_size as u64
}

fn crc32_of(data: &[u8]) -> u32 {
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

fn write_u64(buf: &mut [u8], pos: &mut usize, val: u64) {
    buf[*pos..*pos + 8].copy_from_slice(&val.to_le_bytes());
    *pos += 8;
}

fn write_u32(buf: &mut [u8], pos: &mut usize, val: u32) {
    buf[*pos..*pos + 4].copy_from_slice(&val.to_le_bytes());
    *pos += 4;
}

fn write_u16(buf: &mut [u8], pos: &mut usize, val: u16) {
    buf[*pos..*pos + 2].copy_from_slice(&val.to_le_bytes());
    *pos += 2;
}

fn read_u64(data: &[u8], pos: &mut usize) -> u64 {
    let val = u64::from_le_bytes(data[*pos..*pos + 8].try_into().unwrap());
    *pos += 8;
    val
}

fn read_u32(data: &[u8], pos: &mut usize) -> u32 {
    let val = u32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    val
}

fn read_u16(data: &[u8], pos: &mut usize) -> u16 {
    let val = u16::from_le_bytes(data[*pos..*pos + 2].try_into().unwrap());
    *pos += 2;
    val
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn superblock_round_trip() {
        let sb = Superblock::new(4096, 32768);
        let data = sb.serialize(4096);
        assert_eq!(data.len(), 4096);
        let sb2 = Superblock::deserialize(&data).unwrap();
        assert_eq!(sb2.magic, SUPERBLOCK_MAGIC);
        assert_eq!(sb2.version, FORMAT_VERSION);
        assert_eq!(sb2.sector_size, 4096);
        assert_eq!(sb2.partition_sectors, 32768);
    }

    #[test]
    fn superblock_corrupt_rejected() {
        let sb = Superblock::new(4096, 32768);
        let mut data = sb.serialize(4096);
        data[10] ^= 0xFF; // corrupt a byte
        assert!(Superblock::deserialize(&data).is_none());
    }

    #[test]
    fn entry_record_round_trip() {
        let record = EntryRecord::new("hello".to_string(), b"world_data".to_vec());
        let serialized = record.serialize(4096);
        assert_eq!(serialized.len(), 4096); // padded to sector
        let (parsed, raw_size) = EntryRecord::deserialize(&serialized).unwrap();
        assert_eq!(parsed.key, "hello");
        assert_eq!(parsed.value, b"world_data");
        assert_eq!(raw_size, 12 + 5 + 10); // header + key + value
    }

    #[test]
    fn entry_record_corrupt_rejected() {
        let record = EntryRecord::new("key".to_string(), b"val".to_vec());
        let mut data = record.serialize(4096);
        data[15] ^= 0xFF;
        assert!(EntryRecord::deserialize(&data).is_none());
    }

    #[test]
    fn region_round_trip() {
        let entries = vec![
            ("k1".to_string(), b"v1".to_vec()),
            ("k2".to_string(), b"longer_value".to_vec()),
            ("k3".to_string(), vec![0u8; 1000]),
        ];
        let data = serialize_region(&entries, 42, 4096);
        let (header, parsed) = deserialize_region(&data, 4096).unwrap();
        assert_eq!(header.flush_seq, 42);
        assert_eq!(header.entry_count, 3);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0], ("k1".to_string(), b"v1".to_vec()));
        assert_eq!(parsed[1], ("k2".to_string(), b"longer_value".to_vec()));
        assert_eq!(parsed[2].0, "k3");
        assert_eq!(parsed[2].1.len(), 1000);
    }

    #[test]
    fn pad_to_sector_works() {
        assert_eq!(pad_to_sector(1, 4096), 4096);
        assert_eq!(pad_to_sector(4096, 4096), 4096);
        assert_eq!(pad_to_sector(4097, 4096), 8192);
        assert_eq!(pad_to_sector(0, 4096), 0);
    }
}
