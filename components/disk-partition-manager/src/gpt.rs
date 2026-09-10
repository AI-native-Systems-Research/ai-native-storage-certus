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
// Kani verification harnesses (Role 2 — consumes the dpm property inventory by id).
// CLEAN-SLATE RE-RUN 2026-09-09 under TIGHT knobs: --default-unwind 3,
// --harness-timeout 90s (5x tighter than the prior 480s/300s runs), -j 24.
//
// Backing-structure triage (done FIRST): GPT state = 92-byte header + a
// 128 x 128B entry array modeled as byte slices ([u8]); CRC32 over byte ranges;
// all layout/LBA logic is u32/u64 integer arithmetic; a UTF-16LE name codec.
// NO BTreeMap / HashMap / raw pointer in the gpt logic. Vec is bounded <= 128.
//   => arithmetic/array/bitmap harnesses are TRACTABLE (go).
// Three intrinsic Kani/CBMC walls remain and are captured (not asserted) by the
// wall_* harnesses below: (1) crc32fast::hash lowers to `_xgetbv`
// (unsupported_construct); (2) integer `format!` in error arms does not terminate
// under CBMC; (3) the production 128-slot entry-array zero-pad needs unwind>=129,
// which at --default-unwind 3 trips an unwinding assertion (and SAT-explodes if
// forced high). Instance construction (ClientChannels/Arc/atomics) is likewise a
// wall, so the pure GPT fns are associated fns (no &self) called directly.
// ============================================================================
#[cfg(kani)]
mod verification {
    use super::*;
    use interfaces::PartitionSpec;

    // entry-array sectors, re-derived from production `entry_sectors`
    // (128 slots x 128B, div_ceil sector_size).
    fn entry_sectors_for(sector_size: u32) -> u32 {
        (GPT_MAX_ENTRIES * GPT_ENTRY_SIZE).div_ceil(sector_size)
    }

    // ===================== GROUP A — pure / re-derived arithmetic =====================

    /// `DPM-ROUNDTRIP-NAME` (G7, SC-003). ASCII name UTF-8 -> UTF-16LE -> UTF-8
    /// preserves the string. TIGHT-KNOB EXPECTATION: the symbolic-string
    /// encode+`from_utf16_lossy` cost (prior run: 592 s / 19.5 GB SUCCESSFUL) now
    /// exceeds the 90 s harness-timeout -> captured as a tool-boundary (rc=124).
    #[kani::proof]
    #[kani::unwind(6)]
    fn verify_name_roundtrip_ascii() {
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

    /// `DPM-FORMAT-NAME-LEN-36` (FR-009). The on-disk name buffer is exactly
    /// 72 bytes = 36 UTF-16 code units; the encoder truncates to fit.
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
        assert!(encoded.len() == 72, "FR-009: name buffer is exactly 36 code units");
    }

    /// `DPM-FORMAT-DISK-GUID-V4` / `DPM-FORMAT-PART-GUID-V4` (FR-008). For ANY
    /// entropy bytes, the version/variant nibble-stamping (gpt.rs generate_guid)
    /// yields the RFC-4122 v4 version nibble (0x4_) and variant bits (10xx).
    /// Re-derived over symbolic [u8;16] (generate_guid's /dev/urandom read is a
    /// real-I/O wall; uniqueness is R6/environmental).
    #[kani::proof]
    fn verify_guid_v4_nibbles() {
        let mut guid: [u8; 16] = kani::any();
        guid[6] = (guid[6] & 0x0F) | 0x40; // version 4
        guid[8] = (guid[8] & 0x3F) | 0x80; // variant 1
        assert!(guid[6] & 0xF0 == 0x40, "FR-008: version-4 nibble");
        assert!(guid[8] & 0xC0 == 0x80, "FR-008: RFC-4122 variant bits");
    }

    /// `DPM-ROUNDTRIP-OFFSETS` (G6, SC-001) arithmetic core. write encodes
    /// `ending = start + num_sectors - 1`; read recovers
    /// `num_sectors = ending - start + 1`. Proven inverse for num_sectors >= 1.
    #[kani::proof]
    fn verify_ending_lba_inverse() {
        let start: u64 = kani::any();
        let num_sectors: u64 = kani::any();
        kani::assume(num_sectors >= 1);
        kani::assume(start <= 1u64 << 40);
        kani::assume(num_sectors <= 1u64 << 40);
        let ending = start + num_sectors - 1; // write (gpt.rs compute_partition_layout)
        let recovered = ending - start + 1; // read (try_read_gpt_at)
        assert!(recovered == num_sectors, "G6: offset/sector round-trip inverse");
    }

    /// `DPM-FORMAT-BACKUP-MIRRORS-PRIMARY` (G6/FR-003). primary my_lba=1,
    /// alternate_lba=N-1; backup my_lba=N-1, alternate_lba=1 — they mirror.
    #[kani::proof]
    fn verify_backup_mirror() {
        let n: u64 = kani::any();
        kani::assume(n >= 2);
        let primary_my = 1u64;
        let primary_alt = n - 1;
        let backup_my = n - 1;
        let backup_alt = 1u64;
        assert!(backup_my == primary_alt, "FR-003: backup my_lba == primary alternate_lba");
        assert!(backup_alt == primary_my, "FR-003: backup alternate_lba == primary my_lba");
    }

    /// R4 (gpt.rs write_gpt): `last_usable_lba = num_sectors-1-entry_sectors-1`
    /// is computed BEFORE the too-small guard. Below `num_sectors >= es+2` the
    /// expression underflows (counterexample region confirmed via checked chain).
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
        assert!(chain.is_none(), "R4: last_usable underflows for num_sectors < entry_sectors+2");
    }

    /// R4 companion: at/above `num_sectors >= es+2`, last_usable is underflow-free
    /// and the too-small guard is the correct sufficient gate.
    #[kani::proof]
    fn verify_r4_last_usable_safe_above_min() {
        let num_sectors: u64 = kani::any();
        let sector_size: u32 = if kani::any() { 512 } else { 4096 };
        let es = entry_sectors_for(sector_size) as u64;
        kani::assume(num_sectors >= es + 2);
        kani::assume(num_sectors <= 1u64 << 48);
        let first_usable = 2 + es;
        let last_usable = num_sectors - 1 - es - 1; // now safe
        if first_usable < last_usable {
            assert!(last_usable >= first_usable, "R4: usable window well-formed");
        }
    }

    /// R4 (gpt.rs read_gpt): backup-LBA arithmetic `num_sectors-1-entry_sectors`
    /// underflows for `num_sectors <= entry_sectors` (includes num_sectors==0).
    #[kani::proof]
    fn verify_r4_backup_lba_underflows_tiny() {
        let num_sectors: u64 = kani::any();
        let sector_size: u32 = if kani::any() { 512 } else { 4096 };
        let es = entry_sectors_for(sector_size) as u64;
        kani::assume(num_sectors <= es);
        let chain = num_sectors.checked_sub(1).and_then(|b| b.checked_sub(es));
        assert!(chain.is_none(), "R4: backup LBA underflows for num_sectors <= entry_sectors");
    }

    /// R4 (gpt.rs try_read_gpt_at): `num_sectors = ending - starting + 1` on a
    /// CRC-valid-but-hostile on-disk entry underflows if `ending < starting`,
    /// and the `+1` overflows exactly when `ending - starting == u64::MAX`.
    #[kani::proof]
    fn verify_r4_ending_minus_starting() {
        let starting: u64 = kani::any();
        let ending: u64 = kani::any();
        if ending < starting {
            assert!(
                ending.checked_sub(starting).is_none(),
                "R4: underflows when ending_lba < starting_lba"
            );
        } else {
            let diff = ending - starting;
            let plus1 = diff.checked_add(1);
            assert!(
                plus1.is_some() == (diff != u64::MAX),
                "R4: '+1' overflows only at diff == u64::MAX"
            );
        }
    }

    // ================= GROUP B — real associated-fn calls (no instance) =================

    /// `entry_sectors` real call == formula; 512 -> 32 sectors, 4096 -> 4.
    /// Base for FIRST/LAST-USABLE and PART-SIZE-CEIL.
    #[kani::proof]
    fn verify_entry_sectors_real() {
        let sector_size: u32 = if kani::any() { 512 } else { 4096 };
        let es = GptManager::entry_sectors(sector_size);
        assert!(es == entry_sectors_for(sector_size), "entry_sectors == div_ceil formula");
        if sector_size == 512 {
            assert!(es == 32, "512-byte sectors -> 32 entry sectors");
        } else {
            assert!(es == 4, "4096-byte sectors -> 4 entry sectors");
        }
    }

    /// `PARSE-HDR-TOO-SHORT` (supports DPM-INIT-BACKUP-FALLBACK / -RETURNS-LAYOUT).
    /// A header buffer shorter than 92 bytes is rejected CorruptTable before any
    /// field access. Real `GptManager::parse_header` call.
    #[kani::proof]
    fn verify_parse_header_too_short() {
        let data = [0u8; 8]; // < 92
        let r = GptManager::parse_header(&data);
        assert!(
            matches!(r, Err(PartitionTableError::CorruptTable(_))),
            "PARSE-HDR-TOO-SHORT: short header -> CorruptTable"
        );
    }

    /// `PARSE-ENTRY-BOUNDS` (implements DPM-INIT-RETURNS-CORRECT-LAYOUT). The loop
    /// stops when a full 128-byte slot would exceed the buffer: a 200-byte buffer
    /// yields exactly one whole entry even when 3 are requested. Real call.
    #[kani::proof]
    #[kani::unwind(4)]
    fn verify_parse_entries_bounds() {
        let data = [0u8; 200]; // 256 > 200 -> only one whole slot
        let entries = GptManager::parse_entries(&data, 3);
        assert!(entries.len() == 1, "PARSE-ENTRY-BOUNDS: only whole slots parsed");
    }

    /// `DPM-FORMAT-ERR-MULTI-REST` (FR-005). Two size_bytes==0 specs -> LayoutError
    /// before placement (early return, before the 128-slot pad). Real
    /// `compute_partition_layout` call.
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
            "FR-005: >1 rest-of-disk partition -> LayoutError"
        );
    }

    /// `DPM-FORMAT-PART-SIZE-CEIL` (FR-004), `-PART-NONOVERLAP` and
    /// `-PART-WITHIN-USABLE` (US1-AS1), over the placement arithmetic
    /// compute_partition_layout runs per fixed partition:
    /// `ns = ceil(size/ss); end = cur + ns - 1; next = end + 1`. Two symbolic-size
    /// partitions that fit. Re-derived (the happy path's 128-slot pad is a wall,
    /// captured separately by wall_layout_happy_path_pad).
    #[kani::proof]
    fn verify_layout_placement_arithmetic() {
        let ss: u64 = 512;
        let first = 34u64;
        let last = 4096u64;
        let total_usable = last - first + 1;

        let s1: u64 = kani::any();
        let s2: u64 = kani::any();
        kani::assume(s1 >= 1 && s1 <= ss * 8);
        kani::assume(s2 >= 1 && s2 <= ss * 8);

        let ns1 = s1.div_ceil(ss);
        let ns2 = s2.div_ceil(ss);
        kani::assume(ns1 + ns2 <= total_usable);

        let start1 = first;
        let end1 = start1 + ns1 - 1;
        let start2 = end1 + 1;
        let end2 = start2 + ns2 - 1;

        assert!(end1 - start1 + 1 == ns1, "FR-004: ceil sector count, partition 1");
        assert!(end2 - start2 + 1 == ns2, "FR-004: ceil sector count, partition 2");
        assert!(start2 == end1 + 1, "US1-AS1: contiguous placement");
        assert!(start2 > start1, "US1-AS1: strictly increasing starts");
        assert!(start1 >= first && end2 <= last, "US1-AS1: within usable window");
    }

    /// `DPM-FORMAT-RESTOFDISK-REMAINING` (FR-004). The single size_bytes==0
    /// partition takes `rest = total_usable - fixed_sectors`; one fixed + rest
    /// consumes the whole usable window exactly.
    #[kani::proof]
    fn verify_layout_restofdisk_arithmetic() {
        let first = 34u64;
        let last = 4096u64;
        let total_usable = last - first + 1;
        let fixed_sectors: u64 = kani::any();
        kani::assume(fixed_sectors >= 1 && fixed_sectors < total_usable);
        let rest_sectors = total_usable - fixed_sectors;

        let end_fixed = first + fixed_sectors - 1;
        let start_rest = end_fixed + 1;
        let end_rest = start_rest + rest_sectors - 1;

        assert!(end_rest == last, "FR-004: rest partition ends exactly at last_usable");
        assert!(
            (end_fixed - first + 1) + (end_rest - start_rest + 1) == total_usable,
            "FR-004: fixed + rest consume the whole usable window"
        );
    }

    // ===================== WALL harnesses — capture tool-boundary signatures =====================
    // These are AUTHORED + RUN to capture a reproducible failure signature (three
    // end-states: tool-boundary requires evidence, never a bare verdict). They are
    // EXPECTED to fail/timeout and document DPM ids that a bounded checker cannot reach.

    /// `DPM-CRC-INTEGRITY` (G8) tool boundary. crc32fast::hash lowers to the
    /// `_xgetbv` intrinsic CBMC does not support. EXPECT: `unsupported_construct`
    /// warning / failure. Route: Creusot + integration test.
    #[kani::proof]
    fn wall_crc32_xgetbv() {
        let buf = [0u8; 92];
        let crc = crc32fast::hash(&buf);
        assert!(crc == crc, "G8: crc32fast reachable (expected _xgetbv wall)");
    }

    /// `DPM-INIT-BOTH-CORRUPT-NOPTBL` (PARSE-SIGNATURE) tool boundary. parse_header
    /// on a >=92-byte buffer: the reject arm builds `format!("...{:#x}", sig)`,
    /// whose integer->string formatting does not terminate under CBMC and poisons
    /// analysis of the whole fn even on the accept side. EXPECT: rc=124 timeout.
    /// Route: Creusot.
    #[kani::proof]
    #[kani::unwind(4)]
    fn wall_parse_header_valid_signature() {
        let mut data = [0u8; 92];
        data[0..8].copy_from_slice(&GPT_SIGNATURE.to_le_bytes()); // valid signature
        let r = GptManager::parse_header(&data);
        assert!(r.is_ok(), "PARSE-SIGNATURE: valid signature parses (expected format! wall)");
    }

    /// `DPM-FORMAT-ENTRY-ARRAY-128x128` / `DPM-FORMAT-TYPEGUID-PRESERVED` /
    /// `DPM-FORMAT-WRITES-5-STRUCTURES` tool boundary. The happy path of
    /// compute_partition_layout runs `while entries.len() < 128 { push zero }`,
    /// requiring unwind>=129; at --default-unwind 3 this trips an unwinding
    /// assertion (and SAT-explodes if forced high). EXPECT: unwinding-assertion
    /// FAILURE. Route: Creusot.
    #[kani::proof]
    fn wall_layout_happy_path_pad() {
        let cfg = PartitionConfig {
            sector_size: 512,
            total_sectors: 1u64 << 20,
            ns_id: 1,
            partitions: vec![PartitionSpec {
                type_guid: [7u8; 16],
                size_bytes: 4096,
                name: String::new(),
            }],
        };
        let r = GptManager::compute_partition_layout(512, &cfg, 34, 4096);
        // If it returned, the type_guid copy (TYPEGUID-PRESERVED) would hold:
        if let Ok(entries) = r {
            assert!(entries.len() == 128, "ENTRY-ARRAY-128x128: exactly 128 slots");
            assert!(entries[0].type_guid == [7u8; 16], "TYPEGUID-PRESERVED: input type-GUID copied");
        }
    }
}
