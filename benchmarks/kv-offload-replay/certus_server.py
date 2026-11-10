#!/usr/bin/env python3
"""
certus_server.py — Certus dispatcher mock over Unix domain socket.

Implements the Certus IDispatcher semantics (populate/lookup/check/remove)
backed by in-memory storage. This faithfully mimics what the Rust Certus
dispatcher does minus the actual GPU DMA and SSD persistence.

Protocol (binary, length-prefixed):
    Request:  [1B opcode][8B key (u64 LE)][4B payload_len (u32 LE)][payload]
    Response: [1B status][4B payload_len (u32 LE)][payload]

Opcodes: POPULATE=1, LOOKUP=2, CHECK=3, REMOVE=4
Status:  OK=1, NOT_FOUND=2, ERROR=3

Start:
    python3 certus_server.py [--socket /tmp/certus.sock] [--verbose]
"""

import argparse
import asyncio
import os
import signal
import struct
import sys
import time
from pathlib import Path

# Protocol constants
OP_POPULATE = 1
OP_LOOKUP = 2
OP_CHECK = 3
OP_REMOVE = 4

STATUS_OK = 1
STATUS_NOT_FOUND = 2
STATUS_ERROR = 3

DEFAULT_SOCKET = "/tmp/certus.sock"


class CertusStore:
    """In-memory storage mimicking Certus dispatcher staging buffers."""

    def __init__(self):
        self._store: dict[int, bytes] = {}
        self._stats = {
            "populate": 0,
            "lookup": 0,
            "lookup_miss": 0,
            "check": 0,
            "check_miss": 0,
            "remove": 0,
            "remove_miss": 0,
            "bytes_in": 0,
            "bytes_out": 0,
        }

    def populate(self, key: int, data: bytes) -> bool:
        """Store data under key. Returns False if key already exists."""
        if key in self._store:
            return False
        self._store[key] = data
        self._stats["populate"] += 1
        self._stats["bytes_in"] += len(data)
        return True

    def lookup(self, key: int) -> bytes | None:
        """Retrieve data for key. Returns None if not found."""
        data = self._store.get(key)
        if data is not None:
            self._stats["lookup"] += 1
            self._stats["bytes_out"] += len(data)
        else:
            self._stats["lookup_miss"] += 1
        return data

    def check(self, key: int) -> bool:
        """Check if key exists."""
        exists = key in self._store
        if exists:
            self._stats["check"] += 1
        else:
            self._stats["check_miss"] += 1
        return exists

    def remove(self, key: int) -> bool:
        """Remove key. Returns False if not found."""
        if key in self._store:
            del self._store[key]
            self._stats["remove"] += 1
            return True
        self._stats["remove_miss"] += 1
        return False

    @property
    def stats(self) -> dict:
        return {**self._stats, "entries": len(self._store)}


class CertusServer:
    """Asyncio Unix domain socket server."""

    def __init__(self, socket_path: str, verbose: bool = False):
        self.socket_path = socket_path
        self.verbose = verbose
        self.store = CertusStore()
        self._server = None
        self._connections = 0

    async def handle_client(
        self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter
    ):
        self._connections += 1
        conn_id = self._connections
        if self.verbose:
            print(f"[conn {conn_id}] connected", file=sys.stderr)

        try:
            while True:
                # Read request header: opcode(1) + key(8) + payload_len(4) = 13 bytes
                header = await reader.readexactly(13)
                opcode = header[0]
                key = struct.unpack_from("<Q", header, 1)[0]
                payload_len = struct.unpack_from("<I", header, 9)[0]

                # Read payload if any
                payload = b""
                if payload_len > 0:
                    payload = await reader.readexactly(payload_len)

                # Dispatch
                status, response_data = self._dispatch(opcode, key, payload)

                # Send response: status(1) + response_len(4) + data
                resp_header = struct.pack("<BI", status, len(response_data))
                writer.write(resp_header + response_data)
                await writer.drain()

        except asyncio.IncompleteReadError:
            pass  # Client disconnected
        except Exception as e:
            if self.verbose:
                print(f"[conn {conn_id}] error: {e}", file=sys.stderr)
        finally:
            writer.close()
            await writer.wait_closed()
            if self.verbose:
                print(f"[conn {conn_id}] disconnected", file=sys.stderr)

    def _dispatch(self, opcode: int, key: int, payload: bytes) -> tuple[int, bytes]:
        if opcode == OP_POPULATE:
            ok = self.store.populate(key, payload)
            if ok:
                return STATUS_OK, b""
            else:
                return STATUS_ERROR, b"already_exists"

        elif opcode == OP_LOOKUP:
            data = self.store.lookup(key)
            if data is not None:
                return STATUS_OK, data
            else:
                return STATUS_NOT_FOUND, b""

        elif opcode == OP_CHECK:
            exists = self.store.check(key)
            # Return 1 byte: 0x01 if exists, 0x00 if not
            return STATUS_OK, bytes([0x01 if exists else 0x00])

        elif opcode == OP_REMOVE:
            ok = self.store.remove(key)
            if ok:
                return STATUS_OK, b""
            else:
                return STATUS_NOT_FOUND, b""

        else:
            return STATUS_ERROR, f"unknown opcode: {opcode}".encode()

    async def start(self):
        # Remove stale socket
        if os.path.exists(self.socket_path):
            os.unlink(self.socket_path)

        self._server = await asyncio.start_unix_server(
            self.handle_client, path=self.socket_path
        )
        print(f"Certus server listening on {self.socket_path}", file=sys.stderr)

    async def run_forever(self):
        await self.start()
        try:
            await self._server.serve_forever()
        except asyncio.CancelledError:
            pass
        finally:
            self.print_stats()

    def print_stats(self):
        stats = self.store.stats
        print(f"\n--- Certus Server Stats ---", file=sys.stderr)
        for k, v in stats.items():
            if k.startswith("bytes"):
                print(f"  {k:>16}: {v / (1024*1024):.2f} MB", file=sys.stderr)
            else:
                print(f"  {k:>16}: {v}", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description="Certus dispatcher mock server")
    parser.add_argument(
        "--socket", default=DEFAULT_SOCKET, help=f"UDS path (default: {DEFAULT_SOCKET})"
    )
    parser.add_argument("--verbose", "-v", action="store_true")
    args = parser.parse_args()

    server = CertusServer(args.socket, verbose=args.verbose)

    loop = asyncio.new_event_loop()

    def shutdown(sig):
        print(f"\nReceived {sig.name}, shutting down...", file=sys.stderr)
        for task in asyncio.all_tasks(loop):
            task.cancel()

    for sig in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(sig, shutdown, sig)

    try:
        loop.run_until_complete(server.run_forever())
    except KeyboardInterrupt:
        pass
    finally:
        # Cleanup socket
        if os.path.exists(args.socket):
            os.unlink(args.socket)
        loop.close()


if __name__ == "__main__":
    main()
