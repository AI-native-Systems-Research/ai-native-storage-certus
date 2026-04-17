use crate::block_device::BlockDeviceClient;
use crate::metadata::OnDiskExtentRecord;
use crate::state::SlabDescriptor;
use interfaces::RecoveryResult;

pub(crate) fn recover(
    client: &BlockDeviceClient,
    slabs: &mut [SlabDescriptor],
    ns_id: u32,
) -> Result<RecoveryResult, interfaces::NvmeBlockError> {
    let mut extents_loaded: u64 = 0;
    let mut orphans_cleaned: u64 = 0;
    let mut corrupt_records: u64 = 0;

    let zero = crate::metadata::zero_block();

    for slab in slabs.iter_mut() {
        for slot in 0..slab.num_slots {
            let lba = slab.record_start_lba + slot as u64;
            let block_data = client.read_block(ns_id, lba)?;

            let record = OnDiskExtentRecord { data: block_data };
            let bitmap_set = slab.bitmap.is_set(slot);
            let is_empty = record.is_empty();
            let crc_valid = record.verify_crc();

            match (bitmap_set, is_empty, crc_valid) {
                (true, false, true) => {
                    extents_loaded += 1;
                }
                (true, false, false) | (true, true, _) => {
                    slab.bitmap.clear(slot);
                    client.write_block(ns_id, lba, &zero)?;
                    corrupt_records += 1;
                }
                (false, false, true) => {
                    client.write_block(ns_id, lba, &zero)?;
                    orphans_cleaned += 1;
                }
                (false, _, _) => {}
            }
        }

        let bitmap_blocks = slab.bitmap.serialize_to_blocks();
        for (i, block) in bitmap_blocks.iter().enumerate() {
            let lba = slab.bitmap_start_lba + i as u64;
            client.write_block(ns_id, lba, block)?;
        }
    }

    Ok(RecoveryResult {
        extents_loaded,
        orphans_cleaned,
        corrupt_records,
    })
}
