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
            // Both a CRC mismatch (`CorruptTable`) and a damaged/zeroed primary
            // signature (`NoPartitionTable` from `parse_header`) are recoverable
            // via the backup header — spec FR-003 / US2 scenario 2 requires the
            // backup to be attempted whenever the primary is corrupt. A torn or
            // partial write can damage the signature bytes just as easily as the
            // CRC, so both must fall through rather than propagate. (If the disk
            // is genuinely unformatted, the backup read also yields
            // `NoPartitionTable` and callers such as `initialize_or_format` still
            // treat it as "no table present".)
            Err(PartitionTableError::CorruptTable(_))
            | Err(PartitionTableError::NoPartitionTable(_)) => {
                // Fall through to try backup
            }
            Err(e) => return Err(e),
        }

        // Try backup header at last LBA
        let backup_lba = self.num_sectors - 1;
        let entry_sectors = self.entry_sectors();
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
        let header = self.parse_header(&header_data)?;

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

        let entries = self.parse_entries(&entry_data, header.num_partition_entries);
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
        let entry_sectors = self.entry_sectors();
        let first_usable_lba = 2 + entry_sectors as u64;
        let last_usable_lba = self.num_sectors - 1 - entry_sectors as u64 - 1;

        if first_usable_lba >= last_usable_lba {
            return Err(PartitionTableError::LayoutError(
                "device too small for GPT".into(),
            ));
        }

        // Compute partition layout
        let entries = self.compute_partition_layout(config, first_usable_lba, last_usable_lba)?;

        // Serialize partition entries
        let entry_data = self.serialize_entries(&entries);
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

    fn compute_partition_layout(
        &self,
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
            .map(|p| p.size_bytes.div_ceil(self.sector_size as u64))
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
                spec.size_bytes.div_ceil(self.sector_size as u64)
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

    fn entry_sectors(&self) -> u32 {
        let entry_bytes = GPT_MAX_ENTRIES * GPT_ENTRY_SIZE;
        entry_bytes.div_ceil(self.sector_size)
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

    fn parse_header(&self, data: &[u8]) -> Result<GptHeader, PartitionTableError> {
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

    fn parse_entries(&self, data: &[u8], count: u32) -> Vec<GptEntry> {
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

    fn serialize_entries(&self, entries: &[GptEntry]) -> Vec<u8> {
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
