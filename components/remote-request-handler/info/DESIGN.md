The purpose of this component is to provide an endpoint for other Certus instances to request a remote lookup over an RDMA network. The component should have an active thread (tokio) polling on a specified port for new connection requests (each we term a session). On receiving a connection request, made via TCP/IP (e.g., rdma_cm), an RDMA connection is created to the remote server. The implementation should use an RC connection with send/recv APIs.

TCP/IP port used to bootstrap connections on should be passed as an component initialization function parameter. The port should be different from the main REST API port. A simple protobuf protocol can be used. HTTP/REST is considered too heavyweight. The main functions on the protocol are lookup_batch and close connection.

Each session thread is passed a reference to the IDispatcher interface so that lookup requests can be resolved. For the initial implementation, the call to IDispatch should only be a dummy placeholder which logs a message to the logger. The remote-request caller will provide remote DMA memory addresses and keys (for each member of the lookup batch) so that the remote-request-handler component can issue a DMA sends or writes if preferred to transfer data directly into the caller memory. The lookup API on the remote-request-handler should be asynchronous and batched.

All debugging and console info output should be made to the ILogger interface.

Implementation must include a basic remote test-client program that can be used to test the endpoint.

Implementation must include unit tests.

Implementation should include optional telemetry to collect connection rates and data copy throughput/latency.

A new profile (full-remote) should be created for the certus-server-yaml executive that includes remote-request-handler.
