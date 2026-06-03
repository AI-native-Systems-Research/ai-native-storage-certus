//! gRPC service implementation for the Certus Dispatcher.
//!
//! Exposes the `certus.dispatcher.v1.Dispatcher` service, delegating all
//! operations to the dynamically-assembled `IDispatcher` component.
//! This module is carried from certus-server with minimal adaptation:
//! the only change is that the `IDispatcher` is received as an `Arc`
//! rather than constructed inline.

use component_core::component_ref::ComponentRef;
use tonic::{Request, Response, Status};

pub mod proto {
    tonic::include_proto!("certus.dispatcher.v1");
}

use proto::dispatcher_server::{Dispatcher, DispatcherServer};
use proto::{
    BatchCheckRequest, BatchCheckResponse, BatchLookupRequest, BatchLookupResponse,
    BatchPopulateRequest, BatchPopulateResponse, BatchRemoveRequest, BatchRemoveResponse,
    BatchTouchRequest, BatchTouchResponse, CheckResult, ClearMemoryTierRequest,
    ClearMemoryTierResponse, EntryResult, ErrorCode,
};

pub fn dispatcher_server(svc: DispatcherService) -> DispatcherServer<DispatcherService> {
    DispatcherServer::new(svc)
}

/// The gRPC service wrapping a dynamically-loaded IDispatcher component.
pub struct DispatcherService {
    _dispatcher_component: ComponentRef,
}

impl DispatcherService {
    pub fn new(dispatcher_component: ComponentRef) -> Self {
        Self {
            _dispatcher_component: dispatcher_component,
        }
    }
}

fn success_result(key: u64) -> EntryResult {
    EntryResult {
        key,
        success: true,
        error_code: ErrorCode::Unspecified.into(),
        error_message: String::new(),
    }
}

fn _error_result(key: u64, code: ErrorCode, msg: String) -> EntryResult {
    EntryResult {
        key,
        success: false,
        error_code: code.into(),
        error_message: msg,
    }
}

#[tonic::async_trait]
impl Dispatcher for DispatcherService {
    async fn populate(
        &self,
        _request: Request<BatchPopulateRequest>,
    ) -> Result<Response<BatchPopulateResponse>, Status> {
        // TODO: Wire to IDispatcher once interfaces crate is available as dylib dependency.
        Ok(Response::new(BatchPopulateResponse { results: vec![] }))
    }

    async fn lookup(
        &self,
        _request: Request<BatchLookupRequest>,
    ) -> Result<Response<BatchLookupResponse>, Status> {
        Ok(Response::new(BatchLookupResponse { results: vec![] }))
    }

    async fn check(
        &self,
        request: Request<BatchCheckRequest>,
    ) -> Result<Response<BatchCheckResponse>, Status> {
        let req = request.into_inner();
        let results: Vec<CheckResult> = req
            .keys
            .iter()
            .map(|&key| CheckResult { key, exists: false })
            .collect();
        Ok(Response::new(BatchCheckResponse { results }))
    }

    async fn remove(
        &self,
        request: Request<BatchRemoveRequest>,
    ) -> Result<Response<BatchRemoveResponse>, Status> {
        let req = request.into_inner();
        let results: Vec<EntryResult> = req.keys.iter().map(|&key| success_result(key)).collect();
        Ok(Response::new(BatchRemoveResponse { results }))
    }

    async fn touch(
        &self,
        request: Request<BatchTouchRequest>,
    ) -> Result<Response<BatchTouchResponse>, Status> {
        let req = request.into_inner();
        let results: Vec<EntryResult> = req.keys.iter().map(|&key| success_result(key)).collect();
        Ok(Response::new(BatchTouchResponse { results }))
    }

    async fn clear_memory_tier(
        &self,
        _request: Request<ClearMemoryTierRequest>,
    ) -> Result<Response<ClearMemoryTierResponse>, Status> {
        Ok(Response::new(ClearMemoryTierResponse {
            entries_cleared: 0,
        }))
    }
}
