Component must be written in Rust.

Component must use the components/component-framework as the basis for defining interfaces and receptacles etc.

Define a type CacheKey which is u64.

Maps extent keys (type CacheKey) to the following metadata:
- location, which is either a DmaBuffer (staging buffer), or a block device offset
- extent_manager_id
- size of extent (in 4KiB blocks)
- atomic read reference count
- atomic write reference count
  
Size of the values for the hash table should be kept as small as possible.

Methods provided by IDispatchMap should include:
- create_staging (key, size) to allocate a staging buffer (DmaBuffer). Returns DmaBuffer. extent_manager_id is embedded in the DmaBuffer metadata. Implicitly increments write reference.
lookup by key, return enum type NotExist(), ErrorMismatchSize(), DmaBuffer(ptr), BlockDeviceLocation(offset) or some error condition - this function increments the read reference count for the returned item (and thus lookup will block until any write reference is gone). The read lock is freed, via release_read, after the caller has finished any transfers.
- convert_to_storage (key: CacheKey, offset : u64, block_device_id: u16) - records on-disk storage location
- take_read(key: CacheKey) : waits for writes=0 then increments read
- take_wait(key: CacheKey) : waits for reads=0 and writes=0 then increments writer reference count
- release_read(key: CacheKey) : decrement read ref count
- release_write(key: CacheKey) : decrement write ref count
- downgrade_reference(key: CacheKey) - atomically downgrades from write to read
- remove (key: CacheKey)

Reference counting is used to ensure writes do not interfere or corrupt reads. A read reference can only be taken if the write reference is 0. A write reference can only be taken if the read reference is 0 and write reference is 0.

On initialization, the component recovers cache-blocks on storage by using IExtentManager::for_each_extent to iterate all of the extents and populate the map

Interface should be thread-safe and re-entrant

Info, debug and error logging should use the ILogger interface.