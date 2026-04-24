use component_macros::define_interface;

use crate::iextent_manager::{Extent, ExtentKey, ExtentManagerError};

#[derive(Debug, Clone)]
pub struct FormatParams {
    pub slab_size: u32,
    pub max_element_size: u32,
    pub chunk_size: u32,
    pub block_size: u32,
    pub region_count: u32,
}

pub struct WriteHandle {
    key: ExtentKey,
    offset: u64,
    size: u32,
    publish_fn: Option<Box<dyn FnOnce() -> Result<Extent, ExtentManagerError> + Send>>,
    abort_fn: Option<Box<dyn FnOnce() + Send>>,
}

impl WriteHandle {
    pub fn new(
        key: ExtentKey,
        offset: u64,
        size: u32,
        publish_fn: Box<dyn FnOnce() -> Result<Extent, ExtentManagerError> + Send>,
        abort_fn: Box<dyn FnOnce() + Send>,
    ) -> Self {
        Self {
            key,
            offset,
            size,
            publish_fn: Some(publish_fn),
            abort_fn: Some(abort_fn),
        }
    }

    pub fn key(&self) -> ExtentKey {
        self.key
    }

    pub fn extent_offset(&self) -> u64 {
        self.offset
    }

    pub fn extent_size(&self) -> u32 {
        self.size
    }

    pub fn publish(mut self) -> Result<Extent, ExtentManagerError> {
        let f = self
            .publish_fn
            .take()
            .expect("publish called on consumed handle");
        self.abort_fn.take();
        f()
    }

    pub fn abort(mut self) {
        self.publish_fn.take();
        if let Some(f) = self.abort_fn.take() {
            f();
        }
    }
}

impl Drop for WriteHandle {
    fn drop(&mut self) {
        if let Some(f) = self.abort_fn.take() {
            f();
        }
    }
}

impl std::fmt::Debug for WriteHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteHandle")
            .field("key", &self.key)
            .field("offset", &self.offset)
            .field("size", &self.size)
            .finish()
    }
}

#[cfg(feature = "spdk")]
define_interface! {
    pub IExtentManagerV2 {
        fn set_dma_alloc(&self, alloc: crate::spdk_types::DmaAllocFn);

        fn format(&self, params: FormatParams) -> Result<(), ExtentManagerError>;

        fn initialize(&self) -> Result<(), ExtentManagerError>;

        fn reserve_extent(
            &self,
            key: ExtentKey,
            size: u32,
        ) -> Result<WriteHandle, ExtentManagerError>;

        fn lookup_extent(&self, key: ExtentKey) -> Result<Extent, ExtentManagerError>;

        fn get_extents(&self) -> Vec<Extent>;

        fn for_each_extent(&self, cb: &mut dyn FnMut(&Extent));

        fn remove_extent(&self, key: ExtentKey) -> Result<(), ExtentManagerError>;

        fn checkpoint(&self) -> Result<(), ExtentManagerError>;
    }
}
