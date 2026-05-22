#!/bin/bash
# Generate Python protobuf stubs from the dispatcher proto file.
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROTO_DIR="$SCRIPT_DIR/../proto"
OUT_DIR="$SCRIPT_DIR"

python -m grpc_tools.protoc \
    -I "$PROTO_DIR" \
    --python_out="$OUT_DIR" \
    --grpc_python_out="$OUT_DIR" \
    "$PROTO_DIR/dispatcher.proto"

echo "Generated Python stubs in $OUT_DIR"
