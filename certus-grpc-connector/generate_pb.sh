#!/bin/bash
# Generate Python protobuf stubs from the dispatcher proto file.
# Stubs are written into the package so they import as
# `certus_grpc_connector.dispatcher_pb2`.
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROTO_DIR="$SCRIPT_DIR/../apps/certus-server/proto"
OUT_DIR="$SCRIPT_DIR/certus_grpc_connector"

python -m grpc_tools.protoc \
    -I "$PROTO_DIR" \
    --python_out="$OUT_DIR" \
    --grpc_python_out="$OUT_DIR" \
    "$PROTO_DIR/dispatcher.proto"

# grpc codegen emits `import dispatcher_pb2` (top-level); rewrite to a
# package-relative import so the stubs work when imported as a submodule.
sed -i 's/^import dispatcher_pb2/from . import dispatcher_pb2/' \
    "$OUT_DIR/dispatcher_pb2_grpc.py"

echo "Generated Python stubs in $OUT_DIR"
