//! Centralized interface trait definitions for the Certus component system.
//!
//! This crate defines all component interface traits (`IGreeter`,
//! `ISPDKEnv`, `IBlockDevice`) in one place, allowing components to depend on
//! interface definitions without pulling in implementation crates.
//!
//! SPDK-dependent interfaces and types are gated behind the `spdk` Cargo feature.
//! Interfaces that merely carry opaque pointers or plain data are *not* gated,
//! even where their implementations need SPDK — see `IMemoryTier` and
//! `IDispatchMap`.

mod idispatch_map;
mod idispatcher;
mod ieviction_policy;
mod igpu_services;
mod igreeter;
mod ilogger;
mod imemory_tier;
mod ipartition_table;
mod iremote_lookup;
mod iremote_lookup_rdma_initiator;
mod iremote_lookup_rdma_responder;
mod itransfer_endpoint;
mod izyre;

pub use idispatch_map::CacheKey;
pub use idispatch_map::DispatchMapError;
// `IDispatchMap` and `LookupResult` are SPDK-orthogonal: the module references no
// SPDK type, only raw pointers and plain data. They are ungated so a component can
// consume tri-state residency (and the dispatch map's atomic lookup-and-pin) without
// enabling `interfaces/spdk` and thus requiring `spdk-sys` — the same treatment
// `IMemoryTier` already has.
pub use idispatch_map::IDispatchMap;
pub use idispatch_map::LookupResult;
pub use idispatcher::DispatcherConfig;
pub use idispatcher::DispatcherError;
pub use idispatcher::EvictionEvent;
pub use idispatcher::EvictionReason;
#[cfg(feature = "spdk")]
pub use idispatcher::IDispatcher;
pub use idispatcher::IpcHandle;
pub use idispatcher::LookupOutcome;
pub use idispatcher::ServedBy;
pub use idispatcher::TierEventStats;
pub use ieviction_policy::BlockSemantics;
pub use ieviction_policy::EvictionHandle;
pub use ieviction_policy::EvictionPolicyError;
pub use ieviction_policy::IEvictionPolicy;
pub use ieviction_policy::PoolId;
pub use ieviction_policy::SessionId;
pub use igpu_services::GpuDeviceInfo;
pub use igpu_services::GpuDmaBuffer;
pub use igpu_services::GpuIpcHandle;
pub use igpu_services::GpuStream;
pub use igpu_services::IGpuServices;
pub use igreeter::IGreeter;
pub use ilogger::ILogger;
pub use imemory_tier::IMemoryTier;
pub use imemory_tier::MemoryTierError;
pub use imemory_tier::MemoryTierTelemetrySnapshot;
pub use imemory_tier::{BounceBuffer, BounceError, DEFAULT_BOUNCE_RESERVE};
pub use iremote_lookup::IRemoteLookup;
pub use iremote_lookup::LookupConfig;
pub use iremote_lookup::RemoteLookupError;
pub use iremote_lookup::RemoteServeStats;
pub use iremote_lookup_rdma_initiator::IRemoteLookupRdmaInitiator;
pub use iremote_lookup_rdma_initiator::PushCompletion;
pub use iremote_lookup_rdma_initiator::PushStatus;
pub use iremote_lookup_rdma_initiator::RemoteLookupRdmaInitiatorError;
pub use iremote_lookup_rdma_initiator::RemoteRegion;
pub use iremote_lookup_rdma_responder::{
    ControlChannel, Endpoint, IRemoteLookupRdmaResponder, IRemoteLookupRdmaResponderAdmin,
    LocalRegion, RemoteLookupRdmaResponderError, ResponderCommand, ResponderEvent,
};
// Ungated: the endpoint's receptacles are ILogger and IDispatchMap, both
// available without the `spdk` feature, so the component can be a workspace
// default member.
pub use itransfer_endpoint::DeadlineAction;
pub use itransfer_endpoint::EndpointConfig;
pub use itransfer_endpoint::EntryOutcome;
pub use itransfer_endpoint::FetchCompletion;
pub use itransfer_endpoint::FetchResult;
pub use itransfer_endpoint::ITransferEndpoint;
pub use itransfer_endpoint::ITransferEndpointAdmin;
pub use itransfer_endpoint::LandingRegion;
pub use itransfer_endpoint::LocalSlot;
pub use itransfer_endpoint::TransferContextId;
pub use itransfer_endpoint::TransferEndpointError;
pub use itransfer_endpoint::TransferStats;
pub use izyre::GossipConfig;
pub use izyre::IZyre;
pub use izyre::IZyreNode;
pub use izyre::NodeConfig;
pub use izyre::PeerId;
pub use izyre::ZyreError;
pub use izyre::ZyreEvent;

#[cfg(feature = "spdk")]
pub mod spdk_types;

#[cfg(feature = "spdk")]
mod ispdk_env;

#[cfg(feature = "spdk")]
pub mod iblock_device;
mod iextended_metadata_store;
mod iextent_manager;

#[cfg(feature = "spdk")]
pub use spdk_types::DmaAllocFn;
#[cfg(feature = "spdk")]
pub use spdk_types::{is_spdk_env_active, set_spdk_env_active};
#[cfg(feature = "spdk")]
pub use spdk_types::{BlockDeviceError, DmaBuffer, PciAddress, PciId, SpdkEnvError, VfioDevice};

#[cfg(feature = "spdk")]
pub use ispdk_env::ISPDKEnv;

#[cfg(feature = "spdk")]
pub use iblock_device::{
    ClientChannels, Command, Completion, IBlockDevice, NamespaceInfo, NvmeBlockError, OpHandle,
    ReadWriteStats, TelemetrySnapshot, IO_SIZE_BUCKETS,
};

#[cfg(feature = "spdk")]
pub use iblock_device::IBlockDeviceAdmin;

pub use iextended_metadata_store::{ExtendedMetadataStoreError, IExtendedMetadataStore};
pub use iextent_manager::Extent;
pub use iextent_manager::ExtentKey;
pub use iextent_manager::ExtentManagerError;
pub use iextent_manager::FormatParams;
#[cfg(feature = "spdk")]
pub use iextent_manager::IExtentManager;
pub use iextent_manager::WriteHandle;

pub use ipartition_table::type_guids;
#[cfg(feature = "spdk")]
pub use ipartition_table::IPartitionTable;
pub use ipartition_table::PartitionConfig;
pub use ipartition_table::PartitionInfo;
pub use ipartition_table::PartitionSpec;
pub use ipartition_table::PartitionTable;
pub use ipartition_table::PartitionTableError;
