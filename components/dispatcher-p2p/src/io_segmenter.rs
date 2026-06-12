//! MDTS-aware I/O segmentation for block device operations.

/// An I/O segment representing a portion of a larger transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IoSegment {
    /// Byte offset from the start of the buffer.
    pub buffer_offset: usize,
    /// Starting logical block address on the device.
    pub lba: u64,
    /// Number of bytes in this segment.
    pub length: usize,
}

/// Split a transfer into segments respecting the device's maximum transfer size.
///
/// Returns a `Vec<IoSegment>` where each segment is at most `max_transfer_size`
/// bytes. The `sector_size` is used to compute LBA offsets.
///
/// # Panics
///
/// Panics if `max_transfer_size` is zero or `sector_size` is zero.
pub fn segment_io(
    start_lba: u64,
    total_bytes: usize,
    max_transfer_size: u32,
    sector_size: u32,
) -> Vec<IoSegment> {
    assert!(max_transfer_size > 0, "max_transfer_size must be > 0");
    assert!(sector_size > 0, "sector_size must be > 0");

    if total_bytes == 0 {
        return Vec::new();
    }

    let mts = max_transfer_size as usize;
    let ss = sector_size as usize;
    let mut segments = Vec::with_capacity(total_bytes.div_ceil(mts));
    let mut remaining = total_bytes;
    let mut buffer_offset = 0usize;
    let mut lba = start_lba;

    while remaining > 0 {
        let length = remaining.min(mts);
        segments.push(IoSegment {
            buffer_offset,
            lba,
            length,
        });
        buffer_offset += length;
        lba += (length / ss) as u64;
        remaining -= length;
    }

    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_segment_within_limit() {
        let segs = segment_io(0, 4096, 131072, 4096);
        assert_eq!(segs.len(), 1);
        assert_eq!(
            segs[0],
            IoSegment {
                buffer_offset: 0,
                lba: 0,
                length: 4096,
            }
        );
    }

    #[test]
    fn exact_mdts_boundary() {
        let segs = segment_io(0, 131072, 131072, 4096);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].length, 131072);
    }

    #[test]
    fn splits_across_mdts() {
        let segs = segment_io(0, 256 * 1024, 131072, 4096);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].length, 131072);
        assert_eq!(segs[0].buffer_offset, 0);
        assert_eq!(segs[0].lba, 0);
        assert_eq!(segs[1].length, 131072);
        assert_eq!(segs[1].buffer_offset, 131072);
        assert_eq!(segs[1].lba, 32); // 131072 / 4096 = 32
    }

    #[test]
    fn uneven_split() {
        // 200KB with 128KB MDTS = 128KB + 72KB
        let segs = segment_io(10, 200 * 1024, 131072, 4096);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].length, 131072);
        assert_eq!(segs[0].lba, 10);
        assert_eq!(segs[1].length, 200 * 1024 - 131072);
        assert_eq!(segs[1].lba, 10 + 32);
    }

    #[test]
    fn zero_bytes_returns_empty() {
        let segs = segment_io(0, 0, 131072, 4096);
        assert!(segs.is_empty());
    }

    #[test]
    fn many_segments() {
        // 1 MiB with 128 KiB MDTS = 8 segments
        let segs = segment_io(0, 1024 * 1024, 131072, 4096);
        assert_eq!(segs.len(), 8);
        for (i, seg) in segs.iter().enumerate() {
            assert_eq!(seg.buffer_offset, i * 131072);
            assert_eq!(seg.lba, (i * 32) as u64);
            assert_eq!(seg.length, 131072);
        }
    }

    #[test]
    fn non_zero_start_lba() {
        let segs = segment_io(100, 4096, 131072, 4096);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].lba, 100);
    }

    #[test]
    #[should_panic(expected = "max_transfer_size must be > 0")]
    fn panics_on_zero_mts() {
        segment_io(0, 4096, 0, 4096);
    }

    #[test]
    #[should_panic(expected = "sector_size must be > 0")]
    fn panics_on_zero_sector_size() {
        segment_io(0, 4096, 131072, 0);
    }
}
