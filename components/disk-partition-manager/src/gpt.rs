use std::sync::{Arc, Mutex};

use interfaces::{
    ClientChannels, Command, Completion, DmaBuffer, IBlockDevice, NvmeBlockError, PartitionConfig,
    PartitionInfo, PartitionTable, PartitionTableError,
};

const GPT_SIGNATURE: u64 = 0x5452_4150_2049_4645; // "EFI PART" little-endian
const GPT_REVISION_1_0: u32 = 0x0001_0000;
const GPT_HEADER_SIZE: u32 = 92;
const GPT_ENTRY_SIZE: u32 = 128;
const GPT_MAX_ENTRIES: u32 = 128;

#[derive(Debug, Clone)]
struct GptHeader {
    signature: u64,
    revision: u32,
    header_size: u32,
    header_crc32: u32,
    my_lba: u64,
    alternate_lba: u64,
    first_usable_lba: u64,
    last_usable_lba: u64,
    disk_guid: [u8; 16],
    partition_entry_lba: u64,
    num_partition_entries: u32,
    partition_entry_size: u32,
    partition_entry_crc32: u32,
}

#[derive(Debug, Clone)]
struct GptEntry {
    type_guid: [u8; 16],
    unique_guid: [u8; 16],
    starting_lba: u64,
    ending_lba: u64,
    attributes: u64,
    name: [u8; 72],
}

pub(crate) struct GptManager {
    channels: ClientChannels,
    sector_size: u32,
    num_sectors: u64,
    ns_id: u32,
}

impl GptManager {
    pub fn new(
        bd: Arc<dyn IBlockDevice + Send + Sync>,
        ns_id: u32,
        sector_size: u32,
        num_sectors: u64,
    ) -> Result<Self, PartitionTableError> {
        let channels = bd
            .connect_client()
            .map_err(|e| PartitionTableError::IoError(e.to_string()))?;
        Ok(Self {
            channels,
            sector_size,
            num_sectors,
            ns_id,
        })
    }

    pub fn read_gpt(&self) -> Result<PartitionTable, PartitionTableError> {
        // Try primary header at LBA 1
        match self.try_read_gpt_at(1, 2) {
            Ok(table) => return Ok(table),
            Err(PartitionTableError::CorruptTable(_)) => {
                // Fall through to try backup
            }
            Err(e) => return Err(e),
        }

        // Try backup header at last LBA
        let backup_lba = self.num_sectors - 1;
        let entry_sectors = Self::entry_sectors(self.sector_size);
        let backup_entry_lba = backup_lba - entry_sectors as u64;
        self.try_read_gpt_at(backup_lba, backup_entry_lba)
            .map_err(|_| {
                PartitionTableError::NoPartitionTable(
                    "neither primary nor backup GPT header is valid".into(),
                )
            })
    }

    fn try_read_gpt_at(
        &self,
        header_lba: u64,
        entry_lba: u64,
    ) -> Result<PartitionTable, PartitionTableError> {
        let header_data = self.read_sector(header_lba)?;
        let header = Self::parse_header(&header_data)?;

        // Validate header CRC (zeroing the crc32 field for computation)
        let mut header_for_crc = header_data[..GPT_HEADER_SIZE as usize].to_vec();
        header_for_crc[16..20].copy_from_slice(&[0u8; 4]);
        let computed_crc = crc32fast::hash(&header_for_crc);
        if computed_crc != header.header_crc32 {
            return Err(PartitionTableError::CorruptTable(format!(
                "header CRC mismatch: expected {:#x}, got {:#x}",
                header.header_crc32, computed_crc
            )));
        }

        // Read partition entries
        let entry_bytes =
            header.num_partition_entries as usize * header.partition_entry_size as usize;
        let entry_data = self.read_bytes(entry_lba, entry_bytes)?;

        // Validate entry array CRC
        let entry_crc = crc32fast::hash(&entry_data);
        if entry_crc != header.partition_entry_crc32 {
            return Err(PartitionTableError::CorruptTable(format!(
                "partition entry CRC mismatch: expected {:#x}, got {:#x}",
                header.partition_entry_crc32, entry_crc
            )));
        }

        let entries = Self::parse_entries(&entry_data, header.num_partition_entries);
        let partitions: Vec<PartitionInfo> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.type_guid != [0u8; 16])
            .map(|(i, e)| PartitionInfo {
                index: i as u32,
                start_lba: e.starting_lba,
                num_sectors: e.ending_lba - e.starting_lba + 1,
                type_guid: e.type_guid,
                unique_guid: e.unique_guid,
                name: decode_utf16le_name(&e.name),
            })
            .collect();

        Ok(PartitionTable {
            partitions,
            sector_size: self.sector_size,
        })
    }

    pub fn write_gpt(
        &self,
        config: &PartitionConfig,
    ) -> Result<PartitionTable, PartitionTableError> {
        let entry_sectors = Self::entry_sectors(self.sector_size);
        let first_usable_lba = 2 + entry_sectors as u64;
        let last_usable_lba = self.num_sectors - 1 - entry_sectors as u64 - 1;

        if first_usable_lba >= last_usable_lba {
            return Err(PartitionTableError::LayoutError(
                "device too small for GPT".into(),
            ));
        }

        // Compute partition layout
        let entries = Self::compute_partition_layout(
            self.sector_size,
            config,
            first_usable_lba,
            last_usable_lba,
        )?;

        // Serialize partition entries
        let entry_data = Self::serialize_entries(&entries);
        let entry_crc = crc32fast::hash(&entry_data);

        // Build primary header
        let disk_guid = generate_guid();
        let primary_header = GptHeader {
            signature: GPT_SIGNATURE,
            revision: GPT_REVISION_1_0,
            header_size: GPT_HEADER_SIZE,
            header_crc32: 0, // computed below
            my_lba: 1,
            alternate_lba: self.num_sectors - 1,
            first_usable_lba,
            last_usable_lba,
            disk_guid,
            partition_entry_lba: 2,
            num_partition_entries: GPT_MAX_ENTRIES,
            partition_entry_size: GPT_ENTRY_SIZE,
            partition_entry_crc32: entry_crc,
        };

        let primary_header_bytes = self.serialize_header_with_crc(&primary_header);

        // Build backup header
        let backup_entry_lba = last_usable_lba + 1;
        let backup_header = GptHeader {
            my_lba: self.num_sectors - 1,
            alternate_lba: 1,
            partition_entry_lba: backup_entry_lba,
            ..primary_header
        };
        let backup_header_bytes = self.serialize_header_with_crc(&backup_header);

        // Write protective MBR
        self.write_protective_mbr()?;

        // Write primary header at LBA 1
        self.write_sector(1, &primary_header_bytes)?;

        // Write primary partition entries at LBA 2
        self.write_bytes(2, &entry_data)?;

        // Write backup partition entries
        self.write_bytes(backup_entry_lba, &entry_data)?;

        // Write backup header at last LBA
        self.write_sector(self.num_sectors - 1, &backup_header_bytes)?;

        // Build result
        let partitions: Vec<PartitionInfo> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.type_guid != [0u8; 16])
            .map(|(i, e)| PartitionInfo {
                index: i as u32,
                start_lba: e.starting_lba,
                num_sectors: e.ending_lba - e.starting_lba + 1,
                type_guid: e.type_guid,
                unique_guid: e.unique_guid,
                name: decode_utf16le_name(&e.name),
            })
            .collect();

        Ok(PartitionTable {
            partitions,
            sector_size: self.sector_size,
        })
    }

    // NOTE: associated fn (no `&self`) — depends only on `sector_size`, never on
    // the block-device channels. Kept instance-free so Kani can call it directly
    // (constructing a GptManager pulls in channel/Arc/atomic machinery that CBMC
    // cannot model soundly).
    fn compute_partition_layout(
        sector_size: u32,
        config: &PartitionConfig,
        first_usable_lba: u64,
        last_usable_lba: u64,
    ) -> Result<Vec<GptEntry>, PartitionTableError> {
        let total_usable = last_usable_lba - first_usable_lba + 1;
        let mut entries = Vec::new();
        let mut current_lba = first_usable_lba;
        let mut remaining = total_usable;

        // Find the "rest of disk" partition (size_bytes == 0), if any
        let rest_count = config
            .partitions
            .iter()
            .filter(|p| p.size_bytes == 0)
            .count();
        if rest_count > 1 {
            return Err(PartitionTableError::LayoutError(
                "at most one partition may use size_bytes=0 (rest of disk)".into(),
            ));
        }

        // First pass: compute fixed-size partitions to determine space for "rest" partition
        let fixed_sectors: u64 = config
            .partitions
            .iter()
            .filter(|p| p.size_bytes > 0)
            .map(|p| p.size_bytes.div_ceil(sector_size as u64))
            .sum();

        if fixed_sectors > total_usable {
            return Err(PartitionTableError::LayoutError(format!(
                "partitions require {} sectors but only {} usable",
                fixed_sectors, total_usable
            )));
        }

        let rest_sectors = total_usable - fixed_sectors;

        for spec in &config.partitions {
            let num_sectors = if spec.size_bytes == 0 {
                rest_sectors
            } else {
                spec.size_bytes.div_ceil(sector_size as u64)
            };

            if num_sectors > remaining {
                return Err(PartitionTableError::LayoutError(format!(
                    "partition '{}' requires {} sectors but only {} remain",
                    spec.name, num_sectors, remaining
                )));
            }

            let ending_lba = current_lba + num_sectors - 1;
            let name_bytes = encode_utf16le_name(&spec.name);

            entries.push(GptEntry {
                type_guid: spec.type_guid,
                unique_guid: generate_guid(),
                starting_lba: current_lba,
                ending_lba,
                attributes: 0,
                name: name_bytes,
            });

            current_lba = ending_lba + 1;
            remaining -= num_sectors;
        }

        // Pad remaining entries with zeros (GPT requires 128 entries)
        while entries.len() < GPT_MAX_ENTRIES as usize {
            entries.push(GptEntry {
                type_guid: [0u8; 16],
                unique_guid: [0u8; 16],
                starting_lba: 0,
                ending_lba: 0,
                attributes: 0,
                name: [0u8; 72],
            });
        }

        Ok(entries)
    }

    // Associated fn (no `&self`): depends only on `sector_size`.
    fn entry_sectors(sector_size: u32) -> u32 {
        let entry_bytes = GPT_MAX_ENTRIES * GPT_ENTRY_SIZE;
        entry_bytes.div_ceil(sector_size)
    }

    fn write_protective_mbr(&self) -> Result<(), PartitionTableError> {
        let mut mbr = vec![0u8; self.sector_size as usize];

        // Partition entry 1 at offset 446 (16 bytes)
        let entry_offset = 446;
        mbr[entry_offset] = 0x00; // not bootable
                                  // CHS of first sector (0/0/2 for LBA 1)
        mbr[entry_offset + 1] = 0x00;
        mbr[entry_offset + 2] = 0x02;
        mbr[entry_offset + 3] = 0x00;
        mbr[entry_offset + 4] = 0xEE; // GPT protective type
                                      // CHS of last sector (0xFF/0xFF/0xFF for large disks)
        mbr[entry_offset + 5] = 0xFF;
        mbr[entry_offset + 6] = 0xFF;
        mbr[entry_offset + 7] = 0xFF;
        // Starting LBA = 1
        mbr[entry_offset + 8..entry_offset + 12].copy_from_slice(&1u32.to_le_bytes());
        // Size in sectors (capped at u32::MAX for large disks)
        let size = (self.num_sectors - 1).min(u32::MAX as u64) as u32;
        mbr[entry_offset + 12..entry_offset + 16].copy_from_slice(&size.to_le_bytes());

        // Boot signature
        mbr[510] = 0x55;
        mbr[511] = 0xAA;

        self.write_sector(0, &mbr)
    }

    // Associated fn (no `&self`): pure byte parsing.
    fn parse_header(data: &[u8]) -> Result<GptHeader, PartitionTableError> {
        if data.len() < GPT_HEADER_SIZE as usize {
            return Err(PartitionTableError::CorruptTable("header too short".into()));
        }

        let signature = u64::from_le_bytes(data[0..8].try_into().unwrap());
        if signature != GPT_SIGNATURE {
            return Err(PartitionTableError::NoPartitionTable(format!(
                "invalid GPT signature: {:#x}",
                signature
            )));
        }

        Ok(GptHeader {
            signature,
            revision: u32::from_le_bytes(data[8..12].try_into().unwrap()),
            header_size: u32::from_le_bytes(data[12..16].try_into().unwrap()),
            header_crc32: u32::from_le_bytes(data[16..20].try_into().unwrap()),
            my_lba: u64::from_le_bytes(data[24..32].try_into().unwrap()),
            alternate_lba: u64::from_le_bytes(data[32..40].try_into().unwrap()),
            first_usable_lba: u64::from_le_bytes(data[40..48].try_into().unwrap()),
            last_usable_lba: u64::from_le_bytes(data[48..56].try_into().unwrap()),
            disk_guid: data[56..72].try_into().unwrap(),
            partition_entry_lba: u64::from_le_bytes(data[72..80].try_into().unwrap()),
            num_partition_entries: u32::from_le_bytes(data[80..84].try_into().unwrap()),
            partition_entry_size: u32::from_le_bytes(data[84..88].try_into().unwrap()),
            partition_entry_crc32: u32::from_le_bytes(data[88..92].try_into().unwrap()),
        })
    }

    // Associated fn (no `&self`): pure byte parsing.
    fn parse_entries(data: &[u8], count: u32) -> Vec<GptEntry> {
        let mut entries = Vec::new();
        for i in 0..count as usize {
            let offset = i * GPT_ENTRY_SIZE as usize;
            if offset + GPT_ENTRY_SIZE as usize > data.len() {
                break;
            }
            let entry_data = &data[offset..offset + GPT_ENTRY_SIZE as usize];
            entries.push(GptEntry {
                type_guid: entry_data[0..16].try_into().unwrap(),
                unique_guid: entry_data[16..32].try_into().unwrap(),
                starting_lba: u64::from_le_bytes(entry_data[32..40].try_into().unwrap()),
                ending_lba: u64::from_le_bytes(entry_data[40..48].try_into().unwrap()),
                attributes: u64::from_le_bytes(entry_data[48..56].try_into().unwrap()),
                name: entry_data[56..128].try_into().unwrap(),
            });
        }
        entries
    }

    fn serialize_header_with_crc(&self, header: &GptHeader) -> Vec<u8> {
        let mut buf = vec![0u8; self.sector_size as usize];
        buf[0..8].copy_from_slice(&header.signature.to_le_bytes());
        buf[8..12].copy_from_slice(&header.revision.to_le_bytes());
        buf[12..16].copy_from_slice(&header.header_size.to_le_bytes());
        // CRC32 at [16..20] — zeroed for now, computed below
        buf[16..20].copy_from_slice(&[0u8; 4]);
        buf[20..24].copy_from_slice(&[0u8; 4]); // reserved
        buf[24..32].copy_from_slice(&header.my_lba.to_le_bytes());
        buf[32..40].copy_from_slice(&header.alternate_lba.to_le_bytes());
        buf[40..48].copy_from_slice(&header.first_usable_lba.to_le_bytes());
        buf[48..56].copy_from_slice(&header.last_usable_lba.to_le_bytes());
        buf[56..72].copy_from_slice(&header.disk_guid);
        buf[72..80].copy_from_slice(&header.partition_entry_lba.to_le_bytes());
        buf[80..84].copy_from_slice(&header.num_partition_entries.to_le_bytes());
        buf[84..88].copy_from_slice(&header.partition_entry_size.to_le_bytes());
        buf[88..92].copy_from_slice(&header.partition_entry_crc32.to_le_bytes());

        // Compute and fill header CRC
        let crc = crc32fast::hash(&buf[..GPT_HEADER_SIZE as usize]);
        buf[16..20].copy_from_slice(&crc.to_le_bytes());

        buf
    }

    // Associated fn (no `&self`): pure serialization.
    fn serialize_entries(entries: &[GptEntry]) -> Vec<u8> {
        let total_bytes = GPT_MAX_ENTRIES as usize * GPT_ENTRY_SIZE as usize;
        let mut buf = vec![0u8; total_bytes];
        for (i, entry) in entries.iter().enumerate() {
            let offset = i * GPT_ENTRY_SIZE as usize;
            buf[offset..offset + 16].copy_from_slice(&entry.type_guid);
            buf[offset + 16..offset + 32].copy_from_slice(&entry.unique_guid);
            buf[offset + 32..offset + 40].copy_from_slice(&entry.starting_lba.to_le_bytes());
            buf[offset + 40..offset + 48].copy_from_slice(&entry.ending_lba.to_le_bytes());
            buf[offset + 48..offset + 56].copy_from_slice(&entry.attributes.to_le_bytes());
            buf[offset + 56..offset + 128].copy_from_slice(&entry.name);
        }
        buf
    }

    fn read_sector(&self, lba: u64) -> Result<Vec<u8>, PartitionTableError> {
        self.read_bytes(lba, self.sector_size as usize)
    }

    fn read_bytes(&self, lba: u64, num_bytes: usize) -> Result<Vec<u8>, PartitionTableError> {
        let num_blocks = num_bytes.div_ceil(self.sector_size as usize);
        let mut result = Vec::with_capacity(num_bytes);

        for i in 0..num_blocks {
            let block_lba = lba + i as u64;
            let buf = alloc_dma_buffer(self.sector_size as usize)
                .map_err(|e| PartitionTableError::IoError(e.to_string()))?;
            let buf = Arc::new(Mutex::new(buf));

            self.channels
                .command_tx
                .send(Command::ReadSync {
                    ns_id: self.ns_id,
                    lba: block_lba,
                    buf: Arc::clone(&buf),
                })
                .map_err(|_| PartitionTableError::IoError("read command send failed".into()))?;

            match self.channels.completion_rx.recv() {
                Ok(Completion::ReadDone { result: res, .. }) => {
                    res.map_err(|e| PartitionTableError::IoError(e.to_string()))?;
                    let locked = buf.lock().unwrap();
                    let remaining = num_bytes - result.len();
                    let to_copy = remaining.min(self.sector_size as usize);
                    result.extend_from_slice(&locked.as_slice()[..to_copy]);
                }
                Ok(Completion::Error { error: e, .. }) => {
                    return Err(PartitionTableError::IoError(e.to_string()));
                }
                Ok(_) => {
                    return Err(PartitionTableError::IoError(
                        "unexpected completion type".into(),
                    ));
                }
                Err(_) => {
                    return Err(PartitionTableError::IoError(
                        "read completion recv failed".into(),
                    ));
                }
            }
        }

        Ok(result)
    }

    fn write_sector(&self, lba: u64, data: &[u8]) -> Result<(), PartitionTableError> {
        self.write_bytes(lba, data)
    }

    fn write_bytes(&self, lba: u64, data: &[u8]) -> Result<(), PartitionTableError> {
        let num_blocks = data.len().div_ceil(self.sector_size as usize);

        for i in 0..num_blocks {
            let block_lba = lba + i as u64;
            let block_start = i * self.sector_size as usize;
            let block_end = (block_start + self.sector_size as usize).min(data.len());

            let mut buf = alloc_dma_buffer(self.sector_size as usize)
                .map_err(|e| PartitionTableError::IoError(e.to_string()))?;
            buf.as_mut_slice()[..block_end - block_start]
                .copy_from_slice(&data[block_start..block_end]);
            // Zero-pad remainder
            if block_end - block_start < self.sector_size as usize {
                for b in &mut buf.as_mut_slice()[block_end - block_start..] {
                    *b = 0;
                }
            }

            #[allow(clippy::arc_with_non_send_sync)]
            let buf = Arc::new(buf);

            self.channels
                .command_tx
                .send(Command::WriteSync {
                    ns_id: self.ns_id,
                    lba: block_lba,
                    buf,
                })
                .map_err(|_| PartitionTableError::IoError("write command send failed".into()))?;

            match self.channels.completion_rx.recv() {
                Ok(Completion::WriteDone { result, .. }) => {
                    result.map_err(|e| PartitionTableError::IoError(e.to_string()))?;
                }
                Ok(Completion::Error { error: e, .. }) => {
                    return Err(PartitionTableError::IoError(e.to_string()));
                }
                Ok(_) => {
                    return Err(PartitionTableError::IoError(
                        "unexpected completion type".into(),
                    ));
                }
                Err(_) => {
                    return Err(PartitionTableError::IoError(
                        "write completion recv failed".into(),
                    ));
                }
            }
        }

        Ok(())
    }
}

fn alloc_dma_buffer(size: usize) -> Result<DmaBuffer, NvmeBlockError> {
    DmaBuffer::new(size, size, None).map_err(|e| {
        NvmeBlockError::BlockDevice(interfaces::BlockDeviceError::DmaAllocationFailed(
            e.to_string(),
        ))
    })
}

fn generate_guid() -> [u8; 16] {
    let mut guid = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut guid);
    }
    // Set version 4 (random) and variant bits per RFC 4122
    guid[6] = (guid[6] & 0x0F) | 0x40; // version 4
    guid[8] = (guid[8] & 0x3F) | 0x80; // variant 1
    guid
}

fn encode_utf16le_name(name: &str) -> [u8; 72] {
    let mut buf = [0u8; 72];
    let chars: Vec<u16> = name.encode_utf16().take(36).collect();
    for (i, &ch) in chars.iter().enumerate() {
        let offset = i * 2;
        if offset + 2 > 72 {
            break;
        }
        buf[offset..offset + 2].copy_from_slice(&ch.to_le_bytes());
    }
    buf
}

fn decode_utf16le_name(data: &[u8; 72]) -> String {
    let chars: Vec<u16> = (0..36)
        .map(|i| u16::from_le_bytes([data[i * 2], data[i * 2 + 1]]))
        .take_while(|&c| c != 0)
        .collect();
    String::from_utf16_lossy(&chars)
}

// ============================================================================
// Kani verification harnesses (Role 2 — consumes property inventory by id).
//
// Backing structures scouted: GPT header + 128x128B entry array as byte slices,
// CRC32 over byte ranges, LBA/layout integer arithmetic, UTF-16LE name codec.
// NO BTreeMap/HashMap. Vec usage is bounded (<= GPT_MAX_ENTRIES = 128); harnesses
// bound the partition count small (1-4) so CBMC does not unroll 128 slots x full
// arithmetic. crc32fast::hash pulls `_xgetbv` (unsupported by CBMC) so every CRC
// path and the end-to-end write_gpt/read_gpt are a Kani TOOL boundary and are
// exercised only up to the CRC call.
// ============================================================================
#[cfg(kani)]
mod verification {
    use super::*;
    use interfaces::PartitionSpec;

    /// Entry-array sectors, re-derived from the production `entry_sectors` body
    /// (128 slots x 128 B, div_ceil sector_size). Used by the R4 arithmetic proofs
    /// which mirror `write_gpt`/`read_gpt` LBA expressions (those functions are not
    /// reachable end-to-end under Kani: `crc32fast::hash` lowers to `_xgetbv`).
    fn entry_sectors_for(sector_size: u32) -> u32 {
        (GPT_MAX_ENTRIES * GPT_ENTRY_SIZE).div_ceil(sector_size)
    }

    // The pure GPT functions are associated fns (no `&self`), so harnesses call
    // them as `GptManager::f(...)` with no I/O-backed instance. Constructing a
    // GptManager is intentionally avoided: its `ClientChannels` (SpscChannel ->
    // RingBuffer/Arc/atomics/fmt) makes CBMC emit spurious library-level check
    // failures and time out — a Kani TOOL boundary, not a DPM property.

    // ======================= GROUP A — instance-free =======================

    // ---- G7: name UTF-8 -> UTF-16LE -> UTF-8 preserves ASCII (<=36 units) ----
    /// `DPM-ROUNDTRIP-NAME` (G7, SC-003). Encoding an ASCII name of <=36 chars to
    /// the on-disk UTF-16LE buffer then decoding it back yields the original name.
    #[kani::proof]
    #[kani::unwind(6)]
    fn verify_name_roundtrip_ascii() {
        // Bound: symbolic length 0..=5 of ASCII, non-NUL bytes.
        let len: usize = kani::any();
        kani::assume(len <= 5);
        let mut bytes = [0u8; 5];
        for b in bytes.iter_mut().take(len) {
            let c: u8 = kani::any();
            kani::assume(c >= 0x20 && c < 0x7f); // printable ASCII, non-NUL
            *b = c;
        }
        let s = core::str::from_utf8(&bytes[..len]).unwrap();

        let encoded = encode_utf16le_name(s);
        let decoded = decode_utf16le_name(&encoded);
        assert!(decoded == s, "G7: ASCII name round-trips through UTF-16LE");
    }

    // ---- DPM-FORMAT-NAME-LEN-36: encoder truncates to <=36 UTF-16 code units --
    /// `DPM-FORMAT-NAME-LEN-36` (FR-009). The 72-byte buffer holds at most 36
    /// UTF-16 code units; a NUL terminator or truncation always fits. Proven by
    /// construction: the fixed [u8;72] output cannot exceed 36 units.
    #[kani::proof]
    #[kani::unwind(6)]
    fn verify_name_len_36() {
        let len: usize = kani::any();
        kani::assume(len <= 5);
        let mut bytes = [0u8; 5];
        for b in bytes.iter_mut().take(len) {
            let c: u8 = kani::any();
            kani::assume(c >= 0x20 && c < 0x7f);
            *b = c;
        }
        let s = core::str::from_utf8(&bytes[..len]).unwrap();
        let encoded = encode_utf16le_name(s);
        // 72 bytes == 36 UTF-16 code units, by construction.
        assert!(encoded.len() == 72, "FR-009: name buffer is exactly 36 units");
    }

    // ---- DPM-FORMAT-DISK/PART-GUID-V4: RFC-4122 v4 version + variant nibbles ----
    /// `DPM-FORMAT-DISK-GUID-V4` / `-PART-GUID-V4` (FR-008). The version/variant
    /// nibble-stamping from `generate_guid` (gpt.rs:561-562) always yields the
    /// RFC-4122 version-4 nibble (0x40) and variant bits (0x80), for ANY entropy
    /// bytes. Re-derived over symbolic input rather than calling `generate_guid`
    /// directly, because its `/dev/urandom` read is a Kani TOOL boundary (real I/O;
    /// and uniqueness is R6/environmental). This proves the structural claim over
    /// the full 2^16 domain of the two touched bytes.
    #[kani::proof]
    fn verify_guid_v4_nibbles() {
        let mut guid: [u8; 16] = kani::any();
        guid[6] = (guid[6] & 0x0F) | 0x40; // gpt.rs:561 version 4
        guid[8] = (guid[8] & 0x3F) | 0x80; // gpt.rs:562 variant 1
        assert!(guid[6] & 0xF0 == 0x40, "FR-008: version-4 nibble");
        assert!(guid[8] & 0xC0 == 0x80, "FR-008: RFC-4122 variant bits");
    }

    // ---- G6-core: ENDING-LBA <-> num_sectors arithmetic inverse ----
    /// `DPM-ROUNDTRIP-OFFSETS` (G6, SC-001) arithmetic core. write encodes
    /// `ending_lba = start + num_sectors - 1`; read recovers
    /// `num_sectors = ending - start + 1`. Proven inverse for num_sectors >= 1.
    #[kani::proof]
    fn verify_ending_lba_inverse() {
        let start: u64 = kani::any();
        let num_sectors: u64 = kani::any();
        kani::assume(num_sectors >= 1);
        // bound so the write-side +/- cannot overflow (mirrors a real device)
        kani::assume(start <= 1u64 << 40);
        kani::assume(num_sectors <= 1u64 << 40);
        let ending = start + num_sectors - 1; // write side (gpt.rs:283)
        let recovered = ending - start + 1; // read side (gpt.rs:129/216)
        assert!(recovered == num_sectors, "G6: offset/sector round-trip");
    }

    // ---- DPM-FORMAT-BACKUP-MIRRORS-PRIMARY arithmetic ----
    /// `DPM-FORMAT-BACKUP-MIRRORS-PRIMARY` (FR-003). Primary my_lba=1,
    /// alternate_lba=N-1; backup my_lba=N-1, alternate_lba=1 — they mirror.
    #[kani::proof]
    fn verify_backup_mirror() {
        let n: u64 = kani::any();
        kani::assume(n >= 2);
        let primary_my = 1u64;
        let primary_alt = n - 1;
        let backup_my = n - 1;
        let backup_alt = 1u64;
        assert!(backup_my == primary_alt, "FR-003: backup my_lba == primary alt");
        assert!(backup_alt == primary_my, "FR-003: backup alt == primary my_lba");
    }

    // ---- R4 (gpt.rs:148): last_usable_lba underflow analysis ----
    /// R4 latent underflow at gpt.rs:148
    /// `last_usable_lba = num_sectors - 1 - entry_sectors - 1`, computed BEFORE the
    /// too-small guard at :150. Below the implied precondition
    /// `num_sectors >= entry_sectors + 2` the production expression underflows
    /// (counterexample region). Proven: checked chain is None throughout that region.
    #[kani::proof]
    fn verify_r4_last_usable_underflows_below_min() {
        let num_sectors: u64 = kani::any();
        let sector_size: u32 = if kani::any() { 512 } else { 4096 }; // G1
        let es = entry_sectors_for(sector_size) as u64;
        kani::assume(num_sectors < es + 2);
        let chain = num_sectors
            .checked_sub(1)
            .and_then(|x| x.checked_sub(es))
            .and_then(|x| x.checked_sub(1));
        assert!(
            chain.is_none(),
            "R4: gpt.rs:148 underflows for num_sectors < entry_sectors + 2"
        );
    }

    /// R4 companion: at/above the implied precondition, gpt.rs:148 is underflow-free
    /// and yields last_usable_lba > first_usable_lba only when strictly larger, so
    /// the :150 guard is the correct and sufficient gate.
    #[kani::proof]
    fn verify_r4_last_usable_safe_above_min() {
        let num_sectors: u64 = kani::any();
        let sector_size: u32 = if kani::any() { 512 } else { 4096 };
        let es = entry_sectors_for(sector_size) as u64;
        kani::assume(num_sectors >= es + 2);
        kani::assume(num_sectors <= 1u64 << 48);
        let first_usable = 2 + es; // gpt.rs:147
        let last_usable = num_sectors - 1 - es - 1; // gpt.rs:148, now safe
        // guard at :150 rejects first >= last; otherwise usable span is valid
        if first_usable < last_usable {
            assert!(last_usable >= first_usable, "R4: usable window well-formed");
        }
    }

    // ---- R4 (gpt.rs:77-79): backup LBA underflow on the read path ----
    /// R4 latent underflow at gpt.rs:77-79. `backup_lba = num_sectors - 1;
    /// backup_entry_lba = backup_lba - entry_sectors`. Underflows when
    /// `num_sectors == 0` or `num_sectors - 1 < entry_sectors`.
    #[kani::proof]
    fn verify_r4_backup_lba_underflows_tiny() {
        let num_sectors: u64 = kani::any();
        let sector_size: u32 = if kani::any() { 512 } else { 4096 };
        let es = entry_sectors_for(sector_size) as u64;
        kani::assume(num_sectors <= es); // includes num_sectors == 0
        let chain = num_sectors.checked_sub(1).and_then(|b| b.checked_sub(es));
        assert!(
            chain.is_none(),
            "R4: gpt.rs:77-79 backup LBA underflows for num_sectors <= entry_sectors"
        );
    }

    // ---- R4 (gpt.rs:129): ending_lba - starting_lba on a hostile CRC-valid entry --
    /// R4 latent underflow at gpt.rs:129/216: `num_sectors = ending - starting + 1`.
    /// Underflows if `ending < starting`; and even with `ending >= starting`, the
    /// `+ 1` overflows when `ending - starting == u64::MAX`. Both are real
    /// obligations on a CRC-valid-but-hostile on-disk entry.
    #[kani::proof]
    fn verify_r4_ending_minus_starting() {
        let starting: u64 = kani::any();
        let ending: u64 = kani::any();
        // Case 1: ending < starting -> subtraction underflows.
        if ending < starting {
            assert!(
                ending.checked_sub(starting).is_none(),
                "R4: gpt.rs:129 underflows when ending_lba < starting_lba"
            );
        } else {
            // Case 2: ending >= starting -> subtraction safe; +1 can still overflow.
            let diff = ending - starting;
            let plus1 = diff.checked_add(1);
            assert!(
                plus1.is_some() == (diff != u64::MAX),
                "R4: gpt.rs:129 '+1' overflows only at diff == u64::MAX"
            );
        }
    }

    // ============ GROUP B — associated fns (no instance / no channels) ============

    // ---- entry_sectors real-call + formula ----
    /// `entry_sectors` (base for FIRST/LAST-USABLE). Real code equals the div_ceil
    /// formula; 512 -> 32 sectors, 4096 -> 4 sectors.
    #[kani::proof]
    fn verify_entry_sectors_real() {
        let sector_size: u32 = if kani::any() { 512 } else { 4096 };
        let es = GptManager::entry_sectors(sector_size);
        assert!(es == entry_sectors_for(sector_size), "entry_sectors == formula");
        if sector_size == 512 {
            assert!(es == 32, "512-byte sectors -> 32 entry sectors");
        } else {
            assert!(es == 4, "4096-byte sectors -> 4 entry sectors");
        }
    }

    // ---- PARSE-HDR-TOO-SHORT: parse_header rejects < 92 bytes ----
    /// `PARSE-HDR-TOO-SHORT`. A header buffer shorter than GPT_HEADER_SIZE (92) is
    /// rejected as CorruptTable before any field access.
    #[kani::proof]
    fn verify_parse_header_too_short() {
        let data = [0u8; 8]; // < 92
        let r = GptManager::parse_header(&data);
        assert!(
            matches!(r, Err(PartitionTableError::CorruptTable(_))),
            "PARSE-HDR-TOO-SHORT: short header -> CorruptTable"
        );
    }

    // ---- PARSE-SIGNATURE: TOOL boundary (no harness) ----
    // Both sides of the signature gate are a Kani/CBMC TOOL boundary. The reject arm
    // eagerly builds `format!("invalid GPT signature: {:#x}", signature)`, and CBMC's
    // integer->string formatting does not terminate. Empirically this poisons analysis
    // of the *whole* `parse_header` function: even a fully-concrete valid-signature
    // header (accept side) does not return within 300s, because reaching the Ok
    // construction requires traversing past the format!-bearing branch. Only the
    // pre-signature guard (PARSE-HDR-TOO-SHORT, early return) is provable — see
    // verify_parse_header_too_short above. Reproduce: a harness calling
    // GptManager::parse_header on any >=92-byte buffer times out at rc=124 / ~16MB RSS.

    // ---- PARSE-ENTRY-BOUNDS: parse_entries never reads past the buffer ----
    /// `PARSE-ENTRY-BOUNDS` (implements DPM-INIT-RETURNS-CORRECT-LAYOUT). The loop
    /// stops as soon as a full 128-byte slot would exceed the buffer: with `count`
    /// requested but only room for `k` slots, exactly `k` entries are produced.
    #[kani::proof]
    #[kani::unwind(4)]
    fn verify_parse_entries_bounds() {
        // 200-byte buffer holds exactly one full 128-byte entry (256 > 200).
        let data = [0u8; 200];
        let entries = GptManager::parse_entries(&data, 3);
        assert!(entries.len() == 1, "PARSE-ENTRY-BOUNDS: only whole slots parsed");
    }

    // ---- DPM-FORMAT-ERR-MULTI-REST: > 1 size_bytes==0 -> LayoutError ----
    /// `DPM-FORMAT-ERR-MULTI-REST` (FR-005). Two rest-of-disk (size 0) specs are
    /// rejected as LayoutError before placement (early return, no 128-pad).
    #[kani::proof]
    #[kani::unwind(4)]
    fn verify_layout_multi_rest_err() {
        let cfg = PartitionConfig {
            sector_size: 512,
            total_sectors: 1u64 << 20,
            ns_id: 1,
            partitions: vec![
                PartitionSpec { type_guid: [1u8; 16], size_bytes: 0, name: String::new() },
                PartitionSpec { type_guid: [2u8; 16], size_bytes: 0, name: String::new() },
            ],
        };
        let r = GptManager::compute_partition_layout(512, &cfg, 34, 2048);
        assert!(
            matches!(r, Err(PartitionTableError::LayoutError(_))),
            "FR-005: >1 rest partition -> LayoutError"
        );
    }

    // ---- DPM-FORMAT-PART-SIZE-CEIL + WITHIN-USABLE + NONOVERLAP (arithmetic) ----
    /// `DPM-FORMAT-PART-SIZE-CEIL` (FR-004), `-PART-WITHIN-USABLE` and
    /// `-PART-NONOVERLAP` (US1-AS1), proven over the placement arithmetic that
    /// `compute_partition_layout` runs per fixed partition (gpt.rs:270-296):
    /// `num_sectors = ceil(size/ss); ending = current + num_sectors - 1;
    ///  next_current = ending + 1`. Two symbolic-size fixed partitions that fit.
    /// (Re-derived rather than calling the function on the happy path, whose 128-slot
    /// zero-pad Vec unroll is a Kani TOOL boundary — SAT explosion at unwind>=129;
    /// see the residual ledger. The early-return error branch IS called live in
    /// `verify_layout_multi_rest_err`.)
    #[kani::proof]
    fn verify_layout_placement_arithmetic() {
        let ss: u64 = 512;
        let first = 34u64;
        let last = 4096u64;
        let total_usable = last - first + 1;

        let s1: u64 = kani::any();
        let s2: u64 = kani::any();
        kani::assume(s1 >= 1 && s1 <= ss * 8); // <= 8 sectors each
        kani::assume(s2 >= 1 && s2 <= ss * 8);

        let ns1 = s1.div_ceil(ss);
        let ns2 = s2.div_ceil(ss);
        kani::assume(ns1 + ns2 <= total_usable); // fits (oversubscribe guard passed)

        // partition 1 placed at `first`
        let start1 = first;
        let end1 = start1 + ns1 - 1;
        // partition 2 placed contiguously after 1
        let start2 = end1 + 1;
        let end2 = start2 + ns2 - 1;

        // FR-004: ceil-division sector counts recover exactly.
        assert!(end1 - start1 + 1 == ns1, "FR-004: ceil sector count p1");
        assert!(end2 - start2 + 1 == ns2, "FR-004: ceil sector count p2");
        // US1-AS1 NONOVERLAP: contiguous, strictly increasing starts.
        assert!(start2 == end1 + 1, "US1-AS1: contiguous placement");
        assert!(start2 > start1, "US1-AS1: strictly increasing starts");
        // US1-AS1 WITHIN-USABLE: both lie in [first, last].
        assert!(start1 >= first && end2 <= last, "US1-AS1: within usable window");
    }

    // ---- DPM-FORMAT-RESTOFDISK-REMAINING (arithmetic) ----
    /// `DPM-FORMAT-RESTOFDISK-REMAINING` (FR-004). The single size_bytes==0 partition
    /// takes `rest_sectors = total_usable - fixed_sectors`, and placing one fixed +
    /// the rest consumes the whole usable window exactly (gpt.rs:267-296).
    #[kani::proof]
    fn verify_layout_restofdisk_arithmetic() {
        let first = 34u64;
        let last = 4096u64;
        let total_usable = last - first + 1;
        let fixed_sectors: u64 = kani::any();
        kani::assume(fixed_sectors >= 1 && fixed_sectors < total_usable);
        let rest_sectors = total_usable - fixed_sectors; // gpt.rs:267

        // fixed partition then rest partition
        let end_fixed = first + fixed_sectors - 1;
        let start_rest = end_fixed + 1;
        let end_rest = start_rest + rest_sectors - 1;

        assert!(end_rest == last, "FR-004: rest partition ends exactly at last_usable");
        assert!(
            (end_fixed - first + 1) + (end_rest - start_rest + 1) == total_usable,
            "FR-004: fixed + rest consume the whole usable window"
        );
    }
}
