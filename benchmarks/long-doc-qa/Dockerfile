# LMCache long_doc_qa benchmark — CLIENT image.
#
# long_doc_qa.py is a pure OpenAI-API client (fetched at build time from a
# pinned LMCache commit — see the ADD below — not vendored into this repo):
# it connects to an ALREADY-RUNNING vLLM / OpenAI-compatible server and drives
# a long-document QA workload (generate N docs of --document-length "hi" tokens,
# repeat them --repeat-count times to exercise prefix-cache / KV reuse, then
# report TTFT + throughput for a warmup round and a query round). It does NOT
# start a server and needs NO GPU, NO model weights, NO tokenizer — just
# `openai` + `pandas` (+ `matplotlib` for --visualize). Point it at any backend
# (Certus, LMCache, CPU-offload, plain vLLM) via BASE_URL.
#
# Build (context = this directory):
#   docker build -t long-doc-qa-bench benchmarks/long-doc-qa
#
# Run against a server on the host (server bound to 0.0.0.0:8000):
#   docker run --rm --network host \
#       -e BASE_URL=http://localhost:8000/v1 \
#       -e NUM_DOCUMENTS=16 -e REPEAT_COUNT=4 -e JSON_OUTPUT=1 \
#       -v "$PWD/results:/workspace/results" \
#       long-doc-qa-bench
#   # or pass flags directly (env assembly is skipped):
#   docker run --rm --network host long-doc-qa-bench \
#       --base-url http://localhost:8000/v1 --num-documents 16 --json-output
#
# Source (Apache-2.0): LMCache benchmarks/long_doc_qa/long_doc_qa.py, pinned to
# commit ed56197172aaf22d8806328c30f3923eddfa314c. To bump: change the commit in
# the ADD URL and the sha256 in --checksum (compute with `sha256sum` on the new
# file). The build fails on mismatch, so the pin is enforced.

FROM docker.io/library/python:3.11-slim

# Pinned client deps. matplotlib is only touched under --visualize but is small
# and keeps the image self-sufficient for chart output.
RUN pip install --no-cache-dir \
        "openai>=1.30,<2" \
        "pandas>=2.0,<3" \
        "matplotlib>=3.7,<4"

WORKDIR /workspace
# Fetch the workload client from a pinned LMCache commit, integrity-checked
# against its sha256. --checksum makes the build fail if the remote bytes ever
# change, so this is reproducible without vendoring the file into the repo.
ADD --checksum=sha256:a998c801937023d804d2293da314e39fc71e6b68ecedccde5e3b11adeb251d6c \
    https://raw.githubusercontent.com/LMCache/LMCache/ed56197172aaf22d8806328c30f3923eddfa314c/benchmarks/long_doc_qa/long_doc_qa.py \
    /workspace/long_doc_qa.py
COPY docker-entrypoint.sh /usr/local/bin/long-doc-qa-entrypoint
RUN chmod +x /usr/local/bin/long-doc-qa-entrypoint

# Output CSVs (warmup_round.csv / query_round.csv, + PNGs under --visualize) are
# written to CWD — keep it a mountable dir so results survive the container.
RUN mkdir -p /workspace/results
WORKDIR /workspace/results

# Defaults leave the workload shape to long_doc_qa.py's own argparse defaults
# (8 docs × 20000 tokens, repeat 2, output 100). Only the server target is
# mandatory and it has no default — set BASE_URL (or HOST+PORT) at run time.
# MODEL defaults to "auto": the client resolves it from the server's /models.
ENV PYTHONUNBUFFERED=1 \
    MPLBACKEND=Agg \
    MODEL=auto \
    OPENAI_API_KEY=sk-dummy

ENTRYPOINT ["/usr/local/bin/long-doc-qa-entrypoint"]
