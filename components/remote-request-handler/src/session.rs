//! Per-connection session state machine.
//!
//! Manages the lifecycle of a single RDMA connection: handshake,
//! batch lookup processing, and cleanup.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::protocol::proto;

/// Maximum number of entries allowed in a single batch lookup request.
pub const MAX_BATCH_SIZE: u32 = 64;

/// Session lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Connection event received, not yet handshaked.
    Connecting,
    /// Waiting for or processing handshake message.
    Handshake,
    /// Ready to process lookup batches.
    Active,
    /// Close requested, draining in-flight operations.
    Closing,
    /// All resources released.
    Closed,
}

/// Configuration passed to each session at creation.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Protocol version supported by this handler.
    pub protocol_version: u32,
    /// Maximum entries per batch.
    pub max_batch_size: u32,
}

/// A single RDMA session with a remote Certus node.
pub struct Session {
    state: Mutex<SessionState>,
    config: SessionConfig,
    batches_processed: AtomicU64,
}

impl Session {
    pub fn new(config: SessionConfig) -> Self {
        Self {
            state: Mutex::new(SessionState::Connecting),
            config,
            batches_processed: AtomicU64::new(0),
        }
    }

    /// Returns the current session state.
    pub fn state(&self) -> SessionState {
        *self.state.lock().unwrap()
    }

    /// Transition to a new state. Returns error if transition is invalid.
    pub fn transition(&self, new_state: SessionState) -> Result<(), SessionError> {
        let mut state = self.state.lock().unwrap();
        let valid = matches!(
            (*state, new_state),
            (SessionState::Connecting, SessionState::Handshake)
                | (SessionState::Handshake, SessionState::Active)
                | (SessionState::Handshake, SessionState::Closed)
                | (SessionState::Active, SessionState::Closing)
                | (SessionState::Active, SessionState::Closed)
                | (SessionState::Closing, SessionState::Closed)
                | (SessionState::Connecting, SessionState::Closed)
        );
        if valid {
            *state = new_state;
            Ok(())
        } else {
            Err(SessionError::InvalidTransition(*state, new_state))
        }
    }

    /// Process a handshake request. Returns the response to send back.
    pub fn process_handshake(&self, request: &proto::HandshakeRequest) -> proto::HandshakeResponse {
        self.transition(SessionState::Handshake).ok();

        if request.protocol_version != self.config.protocol_version {
            self.transition(SessionState::Closed).ok();
            return proto::HandshakeResponse {
                accepted: false,
                server_version: self.config.protocol_version,
                max_batch_size: self.config.max_batch_size,
                error_message: format!(
                    "version mismatch: server={}, client={}",
                    self.config.protocol_version, request.protocol_version
                ),
            };
        }

        self.transition(SessionState::Active).ok();
        proto::HandshakeResponse {
            accepted: true,
            server_version: self.config.protocol_version,
            max_batch_size: self.config.max_batch_size,
            error_message: String::new(),
        }
    }

    /// Validate a batch lookup request. Returns error if batch is too large.
    pub fn validate_batch(&self, request: &proto::BatchLookupRequest) -> Result<(), SessionError> {
        if request.entries.len() > self.config.max_batch_size as usize {
            return Err(SessionError::BatchTooLarge(
                request.entries.len(),
                self.config.max_batch_size as usize,
            ));
        }
        if request.entries.is_empty() {
            return Err(SessionError::EmptyBatch);
        }
        Ok(())
    }

    /// Process a batch lookup request. Calls the dispatcher for each key
    /// and constructs the response. RDMA Writes are performed separately.
    ///
    /// `resolve_fn` is called for each CacheKey and returns the data bytes
    /// (or None if not found).
    pub fn process_batch<F>(
        &self,
        request: &proto::BatchLookupRequest,
        mut resolve_fn: F,
    ) -> proto::BatchLookupResponse
    where
        F: FnMut(u64) -> Option<Vec<u8>>,
    {
        let results: Vec<proto::EntryResult> = request
            .entries
            .iter()
            .map(|entry| match resolve_fn(entry.cache_key) {
                Some(data) => {
                    let bytes_written = data.len().min(entry.max_size as usize) as u32;
                    proto::EntryResult {
                        cache_key: entry.cache_key,
                        success: true,
                        bytes_written,
                        error_code: proto::ErrorCode::Unspecified as i32,
                        error_message: String::new(),
                    }
                }
                None => proto::EntryResult {
                    cache_key: entry.cache_key,
                    success: false,
                    bytes_written: 0,
                    error_code: proto::ErrorCode::KeyNotFound as i32,
                    error_message: "key not found".into(),
                },
            })
            .collect();

        self.batches_processed.fetch_add(1, Ordering::Relaxed);

        proto::BatchLookupResponse {
            batch_id: request.batch_id,
            results,
        }
    }

    /// Process a close request. Returns the response to send back.
    pub fn process_close(&self, _request: &proto::CloseRequest) -> proto::CloseResponse {
        self.transition(SessionState::Closing).ok();
        let total = self.batches_processed.load(Ordering::Relaxed);
        self.transition(SessionState::Closed).ok();
        proto::CloseResponse {
            batches_total: total,
        }
    }

    /// Force transition to Closed (e.g., on CM disconnect event).
    pub fn force_close(&self) {
        let mut state = self.state.lock().unwrap();
        *state = SessionState::Closed;
    }

    /// Returns the number of batches processed in this session.
    pub fn batches_processed(&self) -> u64 {
        self.batches_processed.load(Ordering::Relaxed)
    }
}

/// Errors that can occur during session operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    InvalidTransition(SessionState, SessionState),
    BatchTooLarge(usize, usize),
    EmptyBatch,
    VersionMismatch(u32, u32),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTransition(from, to) => {
                write!(f, "invalid state transition: {from:?} -> {to:?}")
            }
            Self::BatchTooLarge(got, max) => {
                write!(f, "batch too large: {got} entries (max {max})")
            }
            Self::EmptyBatch => write!(f, "batch is empty"),
            Self::VersionMismatch(server, client) => {
                write!(f, "version mismatch: server={server}, client={client}")
            }
        }
    }
}

impl std::error::Error for SessionError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> SessionConfig {
        SessionConfig {
            protocol_version: 1,
            max_batch_size: MAX_BATCH_SIZE,
        }
    }

    #[test]
    fn session_initial_state() {
        let session = Session::new(test_config());
        assert_eq!(session.state(), SessionState::Connecting);
    }

    #[test]
    fn session_valid_transitions() {
        let session = Session::new(test_config());
        assert!(session.transition(SessionState::Handshake).is_ok());
        assert_eq!(session.state(), SessionState::Handshake);
        assert!(session.transition(SessionState::Active).is_ok());
        assert_eq!(session.state(), SessionState::Active);
        assert!(session.transition(SessionState::Closing).is_ok());
        assert_eq!(session.state(), SessionState::Closing);
        assert!(session.transition(SessionState::Closed).is_ok());
        assert_eq!(session.state(), SessionState::Closed);
    }

    #[test]
    fn session_invalid_transition() {
        let session = Session::new(test_config());
        assert!(session.transition(SessionState::Active).is_err());
    }

    #[test]
    fn handshake_version_match() {
        let session = Session::new(test_config());
        let req = proto::HandshakeRequest {
            protocol_version: 1,
            client_id: "test".into(),
        };
        let resp = session.process_handshake(&req);
        assert!(resp.accepted);
        assert_eq!(resp.max_batch_size, 64);
        assert_eq!(session.state(), SessionState::Active);
    }

    #[test]
    fn handshake_version_mismatch() {
        let session = Session::new(test_config());
        let req = proto::HandshakeRequest {
            protocol_version: 99,
            client_id: "test".into(),
        };
        let resp = session.process_handshake(&req);
        assert!(!resp.accepted);
        assert_eq!(session.state(), SessionState::Closed);
    }

    #[test]
    fn batch_size_validation_rejects_oversized() {
        let session = Session::new(test_config());
        session.transition(SessionState::Handshake).unwrap();
        session.transition(SessionState::Active).unwrap();

        let entries: Vec<proto::LookupEntry> = (0..65)
            .map(|i| proto::LookupEntry {
                cache_key: i,
                remote_addr: 0x1000 + i * 8,
                rkey: 1,
                max_size: 4096,
            })
            .collect();
        let req = proto::BatchLookupRequest {
            batch_id: 1,
            entries,
        };
        assert!(matches!(
            session.validate_batch(&req),
            Err(SessionError::BatchTooLarge(65, 64))
        ));
    }

    #[test]
    fn batch_size_validation_rejects_empty() {
        let session = Session::new(test_config());
        session.transition(SessionState::Handshake).unwrap();
        session.transition(SessionState::Active).unwrap();

        let req = proto::BatchLookupRequest {
            batch_id: 1,
            entries: vec![],
        };
        assert!(matches!(
            session.validate_batch(&req),
            Err(SessionError::EmptyBatch)
        ));
    }

    #[test]
    fn batch_lookup_with_resolver() {
        let session = Session::new(test_config());
        session.transition(SessionState::Handshake).unwrap();
        session.transition(SessionState::Active).unwrap();

        let entries = vec![
            proto::LookupEntry {
                cache_key: 100,
                remote_addr: 0x2000,
                rkey: 1,
                max_size: 4096,
            },
            proto::LookupEntry {
                cache_key: 200,
                remote_addr: 0x3000,
                rkey: 2,
                max_size: 4096,
            },
            proto::LookupEntry {
                cache_key: 300,
                remote_addr: 0x4000,
                rkey: 3,
                max_size: 4096,
            },
        ];
        let req = proto::BatchLookupRequest {
            batch_id: 42,
            entries,
        };

        let resp = session.process_batch(&req, |key| {
            if key == 100 || key == 300 {
                Some(vec![0xAA; 128])
            } else {
                None
            }
        });

        assert_eq!(resp.batch_id, 42);
        assert_eq!(resp.results.len(), 3);
        assert!(resp.results[0].success);
        assert_eq!(resp.results[0].bytes_written, 128);
        assert!(!resp.results[1].success);
        assert_eq!(
            resp.results[1].error_code,
            proto::ErrorCode::KeyNotFound as i32
        );
        assert!(resp.results[2].success);
        assert_eq!(session.batches_processed(), 1);
    }

    #[test]
    fn close_returns_batch_count() {
        let session = Session::new(test_config());
        session.transition(SessionState::Handshake).unwrap();
        session.transition(SessionState::Active).unwrap();

        let entries = vec![proto::LookupEntry {
            cache_key: 1,
            remote_addr: 0x1000,
            rkey: 1,
            max_size: 4096,
        }];
        let req = proto::BatchLookupRequest {
            batch_id: 1,
            entries,
        };
        session.process_batch(&req, |_| Some(vec![1, 2, 3]));
        session.process_batch(&req, |_| Some(vec![4, 5, 6]));

        let close_resp = session.process_close(&proto::CloseRequest {
            reason: "done".into(),
        });
        assert_eq!(close_resp.batches_total, 2);
        assert_eq!(session.state(), SessionState::Closed);
    }

    #[test]
    fn force_close_from_any_state() {
        let session = Session::new(test_config());
        session.transition(SessionState::Handshake).unwrap();
        session.transition(SessionState::Active).unwrap();
        session.force_close();
        assert_eq!(session.state(), SessionState::Closed);
    }
}
