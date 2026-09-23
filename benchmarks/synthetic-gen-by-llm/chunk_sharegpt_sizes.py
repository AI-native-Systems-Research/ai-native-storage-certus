#!/usr/bin/env python3
"""Split a ShareGPT-format dataset into fixed-size chunks and report file sizes.

Conversations are chunked in the order they appear in the dataset, ``chunk_size``
per file (default 1000); the final chunk is partial. For each chunk we compute
the on-disk size of the JSON array it would serialize to, then report the
largest, smallest, mean, and total — the point being to see how big the biggest
split file would be.

By default nothing is written (measure-only). Pass ``--outdir`` to also write the
chunk files as ``<prefix>NNN.json``.

Sizes are measured with the same JSON encoder used to write, so the reported
per-chunk bytes match what would land on disk. The default encoder is compact
(``separators=(",", ":")``, ``ensure_ascii=False`` → real UTF-8 bytes); the total
across all chunks is printed alongside the source file's own byte size as a
sanity check (they differ only by the source's whitespace/escaping style).
"""

import argparse
import json
import os
import sys


def human(n):
    """Bytes -> human string (GiB/MiB/KiB/B)."""
    f = float(n)
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if f < 1024.0 or unit == "TiB":
            return f"{int(n)} B" if unit == "B" else f"{f:.2f} {unit}"
        f /= 1024.0


def main():
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    ap.add_argument(
        "--source",
        default=os.environ.get("DATASET_PATH", "/home/bdh/kvconn-trace/sharegpt_v3.json"),
        help="ShareGPT-format JSON (top-level array of conversations)",
    )
    ap.add_argument("--chunk-size", type=int, default=1000,
                    help="conversations per file (default 1000)")
    ap.add_argument("--outdir", default=None,
                    help="if given, write chunk files here (else measure only)")
    ap.add_argument("--prefix", default="chunk_",
                    help="output filename prefix (default 'chunk_')")
    ap.add_argument("--ensure-ascii", action="store_true",
                    help="escape non-ASCII (json default); off => UTF-8 bytes")
    ap.add_argument("--indent", type=int, default=None,
                    help="pretty-print with this indent (default: compact)")
    ap.add_argument("--top", type=int, default=10,
                    help="how many largest chunks to list (default 10)")
    args = ap.parse_args()

    if args.chunk_size < 1:
        print("chunk-size must be >= 1", file=sys.stderr)
        return 2

    src_bytes = os.path.getsize(args.source)  # follows symlinks
    print(f"[chunk] loading {args.source} ({human(src_bytes)}) ...",
          file=sys.stderr)
    with open(args.source, encoding="utf-8") as fh:
        data = json.load(fh)
    if not isinstance(data, list):
        print(f"expected a top-level JSON array, got {type(data).__name__}",
              file=sys.stderr)
        return 1

    n = len(data)
    cs = args.chunk_size
    n_files = (n + cs - 1) // cs
    full = n // cs
    remainder = n - full * cs
    print(f"[chunk] {n:,} conversations -> {n_files} files of {cs} "
          f"({full} full, last partial = {remainder or cs})", file=sys.stderr)

    if args.outdir:
        os.makedirs(args.outdir, exist_ok=True)

    # separators: compact unless pretty-printing (indent implies default seps).
    seps = None if args.indent is not None else (",", ":")

    sizes = []  # (file_index, start, end, n_convs, byte_size)
    for idx in range(n_files):
        start = idx * cs
        end = min(start + cs, n)
        chunk = data[start:end]
        text = json.dumps(chunk, ensure_ascii=args.ensure_ascii,
                          indent=args.indent, separators=seps)
        blob = text.encode("utf-8")
        size = len(blob)
        sizes.append((idx, start, end, len(chunk), size))
        if args.outdir:
            width = max(3, len(str(n_files - 1)))
            path = os.path.join(args.outdir, f"{args.prefix}{idx:0{width}d}.json")
            with open(path, "wb") as out:
                out.write(blob)

    total = sum(s[4] for s in sizes)
    largest = max(sizes, key=lambda s: s[4])
    smallest = min(sizes, key=lambda s: s[4])
    mean = total / n_files

    print()
    print(f"files:            {n_files}")
    print(f"largest file:     {human(largest[4])} ({largest[4]:,} bytes)  "
          f"= {args.prefix}{largest[0]} "
          f"(convs [{largest[1]}:{largest[2]}], {largest[3]} convs)")
    print(f"smallest file:    {human(smallest[4])} ({smallest[4]:,} bytes)  "
          f"= {args.prefix}{smallest[0]} ({smallest[3]} convs)")
    print(f"mean file:        {human(mean)}")
    print(f"total (all files):{human(total)}  ({total:,} bytes)")
    print(f"source on disk:   {human(src_bytes)}  ({src_bytes:,} bytes)  "
          f"[differs by source whitespace/escaping]")

    top = sorted(sizes, key=lambda s: s[4], reverse=True)[:args.top]
    print(f"\ntop {len(top)} largest files:")
    print(f"  {'file':>18}  {'convs':>6}  {'size':>12}  range")
    for idx, start, end, k, size in top:
        print(f"  {args.prefix + str(idx):>18}  {k:>6}  {human(size):>12}  "
              f"[{start}:{end}]")

    if args.outdir:
        print(f"\nwrote {n_files} files to {args.outdir}/", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
