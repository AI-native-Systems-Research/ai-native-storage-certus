---
apiVersion: v1
kind: Namespace
metadata:
  name: certus
---
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: certus-server
  namespace: certus
  labels:
    app: certus-server
spec:
  selector:
    matchLabels:
      app: certus-server
  template:
    metadata:
      labels:
        app: certus-server
    spec:
      runtimeClassName: nvidia
      imagePullSecrets:
      - name: artifactory-creds
      hostNetwork: true
      # Share the host IPC namespace so (a) client (vLLM) pods co-located on the
      # same node can see the /dev/shm shmq mailbox this server publishes, and
      # (b) this server can open the CUDA IPC handles those clients export for
      # their KV cache. This is the k8s equivalent of podman `--ipc=host`.
      hostIPC: true
      containers:
      - name: certus
        image: %%CERTUS_REGISTRY%%/%%CERTUS_REPO%%/%%CERTUS_IMAGE%%:%%CERTUS_TAG%%
        args:
        - "--drive-count"
        - "1"
        - "--shm-path"
        - "/dev/shm/certus-shmq"
        - "--channels"
        - "32"
        - "--memory-tier-size"
        - "4G"
        resources:
          limits:
            rdma/rdma_shared_device_a: 1
            hugepages-1Gi: 4Gi
          requests:
            memory: 2Gi
        securityContext:
          privileged: true
        volumeMounts:
        - name: hugepages
          mountPath: /dev/hugepages
        - name: vfio
          mountPath: /dev/vfio
        - name: infiniband
          mountPath: /dev/infiniband
        # Host /dev/shm holds the shmq mailbox (/dev/shm/certus-shmq). Mounting
        # the host directory (rather than the pod's private tmpfs) is what lets
        # a co-located client pod that mounts the same host path reach it.
        - name: dev-shm
          mountPath: /dev/shm
      volumes:
      - name: hugepages
        emptyDir:
          medium: HugePages-1Gi
      - name: vfio
        hostPath:
          path: /dev/vfio
          type: Directory
      - name: infiniband
        hostPath:
          path: /dev/infiniband
          type: Directory
      - name: dev-shm
        hostPath:
          path: /dev/shm
          type: Directory
      nodeSelector:
        certus.ai/worker: "true"
# NOTE: there is no client-facing Service. The shmq control transport is a
# node-local /dev/shm mailbox, not a network endpoint — a client (vLLM) pod
# reaches this server only by being scheduled on the SAME node with
# `hostIPC: true` and the same `/dev/shm` hostPath mount. Cross-node
# server-to-server peer discovery (for remote-lookup) is handled by zyre over
# the host network, so no dedicated peer Service is needed either.
