# Derived image: stock vLLM plus the certus shmq connector, for llm-d's
# modelserver pods.
#
# Build from the REPO ROOT so certus-shmq-connector is in context:
#   deploy/llm-d/a30-certus/build-image-and-push.sh
# or by hand:
#   docker build -f deploy/llm-d/a30-certus/Dockerfile -t <image> .
#
# This is NOT deploy/k8s/Dockerfile -- that one builds certus-server itself.
# This builds the vLLM *client* image that talks to a certus server over the
# /dev/shm mailbox.
ARG VLLM_VERSION=v0.30.0
FROM docker.io/vllm/vllm-openai:${VLLM_VERSION}

# The connector is pure Python (ctypes + mmap; no compiled client, no wire
# codegen). torch and numpy already ship in the base image, so --no-deps avoids
# resolving a second torch on top of the CUDA build.
COPY certus-shmq-connector /opt/certus-shmq-connector
RUN pip install --no-cache-dir --no-deps /opt/certus-shmq-connector

# Import the package (not .spec, which pulls in vllm.v1.kv_offload via the compat
# ladder and is better exercised at runtime against a real engine). ring.py
# asserts x86_64 on import, so this also fails the build early on a non-x86
# builder -- the ring relies on x86-TSO store ordering.
RUN python3 -c "import certus_shmq_connector, certus_shmq_connector.ring; \
print('certus-shmq-connector OK:', certus_shmq_connector.__file__)"
