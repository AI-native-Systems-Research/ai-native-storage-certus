//! Protobuf message encode/decode wrappers.
//!
//! Provides helpers for serializing and deserializing the RequestMessage
//! and ResponseMessage envelope types used over RDMA Send/Recv.

use prost::Message;

/// Generated protobuf types.
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/certus.remote_request.v1.rs"));
}

/// Encode a ResponseMessage into a byte buffer.
pub fn encode_response(msg: &proto::ResponseMessage) -> Vec<u8> {
    msg.encode_to_vec()
}

/// Decode a RequestMessage from a byte buffer.
pub fn decode_request(buf: &[u8]) -> Result<proto::RequestMessage, prost::DecodeError> {
    proto::RequestMessage::decode(buf)
}

/// Encode a RequestMessage into a byte buffer (used by client).
pub fn encode_request(msg: &proto::RequestMessage) -> Vec<u8> {
    msg.encode_to_vec()
}

/// Decode a ResponseMessage from a byte buffer (used by client).
pub fn decode_response(buf: &[u8]) -> Result<proto::ResponseMessage, prost::DecodeError> {
    proto::ResponseMessage::decode(buf)
}

/// Helper to wrap a HandshakeResponse into a ResponseMessage envelope.
pub fn handshake_response(resp: proto::HandshakeResponse) -> proto::ResponseMessage {
    proto::ResponseMessage {
        payload: Some(proto::response_message::Payload::Handshake(resp)),
    }
}

/// Helper to wrap a BatchLookupResponse into a ResponseMessage envelope.
pub fn lookup_response(resp: proto::BatchLookupResponse) -> proto::ResponseMessage {
    proto::ResponseMessage {
        payload: Some(proto::response_message::Payload::Lookup(resp)),
    }
}

/// Helper to wrap a CloseResponse into a ResponseMessage envelope.
pub fn close_response(resp: proto::CloseResponse) -> proto::ResponseMessage {
    proto::ResponseMessage {
        payload: Some(proto::response_message::Payload::Close(resp)),
    }
}

/// Helper to wrap a HandshakeRequest into a RequestMessage envelope.
pub fn handshake_request(req: proto::HandshakeRequest) -> proto::RequestMessage {
    proto::RequestMessage {
        payload: Some(proto::request_message::Payload::Handshake(req)),
    }
}

/// Helper to wrap a BatchLookupRequest into a RequestMessage envelope.
pub fn lookup_request(req: proto::BatchLookupRequest) -> proto::RequestMessage {
    proto::RequestMessage {
        payload: Some(proto::request_message::Payload::Lookup(req)),
    }
}

/// Helper to wrap a CloseRequest into a RequestMessage envelope.
pub fn close_request(req: proto::CloseRequest) -> proto::RequestMessage {
    proto::RequestMessage {
        payload: Some(proto::request_message::Payload::Close(req)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_handshake_request() {
        let req = handshake_request(proto::HandshakeRequest {
            protocol_version: 1,
            client_id: "test-client".into(),
        });
        let encoded = encode_request(&req);
        let decoded = decode_request(&encoded).unwrap();
        match decoded.payload {
            Some(proto::request_message::Payload::Handshake(h)) => {
                assert_eq!(h.protocol_version, 1);
                assert_eq!(h.client_id, "test-client");
            }
            _ => panic!("expected handshake payload"),
        }
    }

    #[test]
    fn roundtrip_lookup_request() {
        let entries = vec![
            proto::LookupEntry {
                cache_key: 42,
                remote_addr: 0xDEAD_BEEF,
                rkey: 7,
                max_size: 4096,
            },
            proto::LookupEntry {
                cache_key: 99,
                remote_addr: 0xCAFE_BABE,
                rkey: 8,
                max_size: 2048,
            },
        ];
        let req = lookup_request(proto::BatchLookupRequest {
            batch_id: 5,
            entries,
        });
        let encoded = encode_request(&req);
        let decoded = decode_request(&encoded).unwrap();
        match decoded.payload {
            Some(proto::request_message::Payload::Lookup(l)) => {
                assert_eq!(l.batch_id, 5);
                assert_eq!(l.entries.len(), 2);
                assert_eq!(l.entries[0].cache_key, 42);
                assert_eq!(l.entries[1].rkey, 8);
            }
            _ => panic!("expected lookup payload"),
        }
    }

    #[test]
    fn roundtrip_close_request() {
        let req = close_request(proto::CloseRequest {
            reason: "shutting down".into(),
        });
        let encoded = encode_request(&req);
        let decoded = decode_request(&encoded).unwrap();
        match decoded.payload {
            Some(proto::request_message::Payload::Close(c)) => {
                assert_eq!(c.reason, "shutting down");
            }
            _ => panic!("expected close payload"),
        }
    }

    #[test]
    fn roundtrip_handshake_response() {
        let resp = handshake_response(proto::HandshakeResponse {
            accepted: true,
            server_version: 1,
            max_batch_size: 64,
            error_message: String::new(),
        });
        let encoded = encode_response(&resp);
        let decoded = decode_response(&encoded).unwrap();
        match decoded.payload {
            Some(proto::response_message::Payload::Handshake(h)) => {
                assert!(h.accepted);
                assert_eq!(h.max_batch_size, 64);
            }
            _ => panic!("expected handshake response payload"),
        }
    }

    #[test]
    fn roundtrip_lookup_response() {
        let resp = lookup_response(proto::BatchLookupResponse {
            batch_id: 10,
            results: vec![proto::EntryResult {
                cache_key: 42,
                success: true,
                bytes_written: 256,
                error_code: proto::ErrorCode::Unspecified as i32,
                error_message: String::new(),
            }],
        });
        let encoded = encode_response(&resp);
        let decoded = decode_response(&encoded).unwrap();
        match decoded.payload {
            Some(proto::response_message::Payload::Lookup(l)) => {
                assert_eq!(l.batch_id, 10);
                assert_eq!(l.results[0].bytes_written, 256);
            }
            _ => panic!("expected lookup response payload"),
        }
    }

    #[test]
    fn decode_invalid_bytes() {
        let garbage = vec![0xFF, 0xFE, 0xFD, 0xFC];
        // prost is lenient with unknown fields, but let's verify it doesn't panic
        let result = decode_request(&garbage);
        // May succeed with empty payload or fail - either is acceptable
        if let Ok(msg) = result {
            // If it decodes, payload should be None (no valid oneof)
            let _ = msg.payload;
        }
    }
}
