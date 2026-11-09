---
apiVersion: v1
kind: Namespace
metadata:
  name: %%CERTUS_NAMESPACE%%
---
# The live remote-lookup switch.
#
# The pod spec REFERENCES this key rather than carrying the value, so the group
# is neither baked into the image (the Dockerfile never mentions it; the binary
# only reads the variable) nor embedded in the workload spec. Flip it with
#
#   kubectl -n %%CERTUS_NAMESPACE%% edit configmap certus-config
#   kubectl -n %%CERTUS_NAMESPACE%% rollout restart daemonset -l app=certus-server
#
# A ConfigMap consumed via `env` is injected at pod creation, so it does not
# auto-restart pods -- the explicit rollout above is required, which is what you
# want between two measurement runs anyway.
#
# Missing key or missing ConfigMap mean remote-lookup is off: the variable is
# unset and each instance forms its own isolated single-node group. A key that is
# present but BLANK is not the same thing -- it is a startup error, because the
# only ways to produce it are a broken template substitution or config pipeline,
# where the intent was to join a cluster.
apiVersion: v1
kind: ConfigMap
metadata:
  name: certus-config
  namespace: %%CERTUS_NAMESPACE%%
  labels:
    app: certus-server
# A flow mapping so the renderer emits either an empty map or the single key in
# one substitution. The key must be ABSENT to mean "off", not blank: absence is
# handled by the keyRef's `optional: true` -- pure Kubernetes semantics, with no
# dependence on how the server treats a blank value -- whereas a blank value is
# a startup error, because it can only come from a broken substitution or config
# pipeline and silently isolating would turn a remote-lookup-ON run into an OFF
# one that looks like "remote-lookup did not help".
data: %%CERTUS_RL_GROUP_DATA%%
---
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: certus-server
  namespace: %%CERTUS_NAMESPACE%%
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
        env:
        # Resolved from the certus-config ConfigMap, not from this spec -- see the
        # ConfigMap above for the switch and how to flip it. optional: true means
        # a missing ConfigMap or key leaves the variable unset, which is the
        # "remote-lookup off" case.
        - name: CERTUS_RL_GROUP
          valueFrom:
            configMapKeyRef:
              name: certus-config
              key: rl-group
              optional: true
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
        %%CERTUS_NODE_SELECTOR_KEY%%: "%%CERTUS_NODE_SELECTOR_VALUE%%"
# NOTE: there is no client-facing Service. The shmq control transport is a
# node-local /dev/shm mailbox, not a network endpoint — a client (vLLM) pod
# reaches this server only by being scheduled on the SAME node with
# `hostIPC: true` and the same `/dev/shm` hostPath mount. Cross-node
# server-to-server peer discovery (for remote-lookup) is handled by zyre over
# the host network, so no dedicated peer Service is needed either.
