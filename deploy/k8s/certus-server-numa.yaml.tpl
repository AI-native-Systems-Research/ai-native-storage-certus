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
# NUMA 0 DaemonSet (cluster-wide)
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: certus-server-numa0
  namespace: %%CERTUS_NAMESPACE%%
  labels:
    app: certus-server
    app.kubernetes.io/instance: numa0
spec:
  selector:
    matchLabels:
      app: certus-server
      app.kubernetes.io/instance: numa0
  template:
    metadata:
      labels:
        app: certus-server
        app.kubernetes.io/instance: numa0
    spec:
      runtimeClassName: nvidia
      imagePullSecrets:
      - name: artifactory-creds
      hostNetwork: true
      # Share the host IPC namespace + /dev/shm so co-located client (vLLM) pods
      # can reach this instance's shmq mailbox and this server can open their
      # CUDA IPC handles (k8s equivalent of podman `--ipc=host`).
      hostIPC: true
      initContainers:
      - name: discover-drives
        image: busybox:latest
        command: ["/bin/sh", "-c"]
        args:
        - |
          NUMA=$CERTUS_NUMA_ID
          for dev in /sys/bus/pci/devices/*; do
            class=$(cat $dev/class 2>/dev/null)
            numa_node=$(cat $dev/numa_node 2>/dev/null)
            if [ "$class" = "0x010802" ] && [ "$numa_node" = "$NUMA" ]; then
              basename $dev
            fi
          done | paste -sd, > /config/drives.txt
          echo "NUMA $NUMA drives: $(cat /config/drives.txt)"
        env:
        - name: CERTUS_NUMA_ID
          value: "0"
        volumeMounts:
        - name: config
          mountPath: /config
        - name: sysfs
          mountPath: /sys
          readOnly: true
      containers:
      - name: certus
        image: %%CERTUS_REGISTRY%%/%%CERTUS_REPO%%/%%CERTUS_IMAGE%%:%%CERTUS_TAG%%
        command: ["/bin/sh", "-c"]
        args:
        - |
          ARGS=""
          for dev in $(cat /config/drives.txt | tr ',' ' '); do
            ARGS="$ARGS --device-pci $dev"
          done
          # Pin this instance to the GPU in its own NUMA domain.
          #
          # Otherwise the server serves a client on a non-zero host GPU through
          # device 0 by peer access across the socket interconnect. It trusts the
          # client's reported gpu_device_id, which is always 0 because the device
          # plugin exposes only the assigned GPU inside a pod, so it selects the
          # wrong device and CUDA_IPC_MEM_LAZY_ENABLE_PEER_ACCESS makes that
          # succeed rather than fail. Measured on an A30 pair: 0.05 GiB/s that way
          # versus 2.10 GiB/s same-device, a 43x penalty. Making the NUMA-local
          # GPU the only visible one makes the client's "0" true.
          #
          # Derived, not hardcoded: NUMA id and CUDA ordinal are not the same
          # mapping on every node. A two-GPU node may have GPU0 on NUMA 0 and GPU1
          # on NUMA 1, so ordinal == NUMA id; a one-GPU node whose GPU sits on
          # NUMA 1 has it as ordinal 0, where that equality breaks. A UUID
          # sidesteps ordinals entirely. An instance with no NUMA-local GPU
          # (peer-only) leaves this unset and is unaffected.
          if [ -z "${CUDA_VISIBLE_DEVICES:-}" ]; then
            for pair in $(nvidia-smi --query-gpu=uuid,pci.bus_id --format=csv,noheader | tr -d '[:blank:]'); do
              uuid=${pair%%,*}
              bdf=${pair#*,}
              sysfs=$(printf '%s' "$bdf" | sed 's/^0000//' | tr 'A-Z' 'a-z')
              if [ "$(cat /sys/bus/pci/devices/$sysfs/numa_node 2>/dev/null)" = "$CERTUS_NUMA_ID" ]; then
                export CUDA_VISIBLE_DEVICES="$uuid"
                echo "certus: pinned to NUMA-$CERTUS_NUMA_ID GPU $bdf ($uuid)"
                break
              fi
            done
            if [ -z "${CUDA_VISIBLE_DEVICES:-}" ]; then
              echo "certus: no GPU in NUMA domain $CERTUS_NUMA_ID; CUDA_VISIBLE_DEVICES left unset"
            fi
          fi
          exec certus-server-yaml $ARGS --shm-path ${CERTUS_SHM_PATH} --channels 32 --memory-tier-size 4G
        # Each NUMA instance publishes a DISTINCT mailbox on the shared host
        # /dev/shm; a client selects an instance by pointing at its shm path.
        env:
        - name: CERTUS_SHM_PATH
          value: "/dev/shm/certus-shmq-numa0"
        - name: CERTUS_NUMA_ID
          value: "0"
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
            memory: 1Gi
        securityContext:
          privileged: true
        volumeMounts:
        - name: config
          mountPath: /config
          readOnly: true
        - name: hugepages
          mountPath: /dev/hugepages
        - name: vfio
          mountPath: /dev/vfio
        - name: infiniband
          mountPath: /dev/infiniband
        - name: dev-shm
          mountPath: /dev/shm
        # Read-only, for the NUMA->GPU derivation above.
        - name: sysfs
          mountPath: /sys
          readOnly: true
      volumes:
      - name: config
        emptyDir: {}
      - name: sysfs
        hostPath:
          path: /sys
          type: Directory
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
---
# NUMA 1 DaemonSet (cluster-wide)
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: certus-server-numa1
  namespace: %%CERTUS_NAMESPACE%%
  labels:
    app: certus-server
    app.kubernetes.io/instance: numa1
spec:
  selector:
    matchLabels:
      app: certus-server
      app.kubernetes.io/instance: numa1
  template:
    metadata:
      labels:
        app: certus-server
        app.kubernetes.io/instance: numa1
    spec:
      runtimeClassName: nvidia
      imagePullSecrets:
      - name: artifactory-creds
      hostNetwork: true
      # Share the host IPC namespace + /dev/shm so co-located client (vLLM) pods
      # can reach this instance's shmq mailbox and this server can open their
      # CUDA IPC handles (k8s equivalent of podman `--ipc=host`).
      hostIPC: true
      initContainers:
      - name: discover-drives
        image: busybox:latest
        command: ["/bin/sh", "-c"]
        args:
        - |
          NUMA=$CERTUS_NUMA_ID
          for dev in /sys/bus/pci/devices/*; do
            class=$(cat $dev/class 2>/dev/null)
            numa_node=$(cat $dev/numa_node 2>/dev/null)
            if [ "$class" = "0x010802" ] && [ "$numa_node" = "$NUMA" ]; then
              basename $dev
            fi
          done | paste -sd, > /config/drives.txt
          echo "NUMA $NUMA drives: $(cat /config/drives.txt)"
        env:
        - name: CERTUS_NUMA_ID
          value: "1"
        volumeMounts:
        - name: config
          mountPath: /config
        - name: sysfs
          mountPath: /sys
          readOnly: true
      containers:
      - name: certus
        image: %%CERTUS_REGISTRY%%/%%CERTUS_REPO%%/%%CERTUS_IMAGE%%:%%CERTUS_TAG%%
        command: ["/bin/sh", "-c"]
        args:
        - |
          ARGS=""
          for dev in $(cat /config/drives.txt | tr ',' ' '); do
            ARGS="$ARGS --device-pci $dev"
          done
          # Pin this instance to the GPU in its own NUMA domain.
          #
          # Otherwise the server serves a client on a non-zero host GPU through
          # device 0 by peer access across the socket interconnect. It trusts the
          # client's reported gpu_device_id, which is always 0 because the device
          # plugin exposes only the assigned GPU inside a pod, so it selects the
          # wrong device and CUDA_IPC_MEM_LAZY_ENABLE_PEER_ACCESS makes that
          # succeed rather than fail. Measured on an A30 pair: 0.05 GiB/s that way
          # versus 2.10 GiB/s same-device, a 43x penalty. Making the NUMA-local
          # GPU the only visible one makes the client's "0" true.
          #
          # Derived, not hardcoded: NUMA id and CUDA ordinal are not the same
          # mapping on every node. A two-GPU node may have GPU0 on NUMA 0 and GPU1
          # on NUMA 1, so ordinal == NUMA id; a one-GPU node whose GPU sits on
          # NUMA 1 has it as ordinal 0, where that equality breaks. A UUID
          # sidesteps ordinals entirely. An instance with no NUMA-local GPU
          # (peer-only) leaves this unset and is unaffected.
          if [ -z "${CUDA_VISIBLE_DEVICES:-}" ]; then
            for pair in $(nvidia-smi --query-gpu=uuid,pci.bus_id --format=csv,noheader | tr -d '[:blank:]'); do
              uuid=${pair%%,*}
              bdf=${pair#*,}
              sysfs=$(printf '%s' "$bdf" | sed 's/^0000//' | tr 'A-Z' 'a-z')
              if [ "$(cat /sys/bus/pci/devices/$sysfs/numa_node 2>/dev/null)" = "$CERTUS_NUMA_ID" ]; then
                export CUDA_VISIBLE_DEVICES="$uuid"
                echo "certus: pinned to NUMA-$CERTUS_NUMA_ID GPU $bdf ($uuid)"
                break
              fi
            done
            if [ -z "${CUDA_VISIBLE_DEVICES:-}" ]; then
              echo "certus: no GPU in NUMA domain $CERTUS_NUMA_ID; CUDA_VISIBLE_DEVICES left unset"
            fi
          fi
          exec certus-server-yaml $ARGS --shm-path ${CERTUS_SHM_PATH} --channels 32 --memory-tier-size 4G
        # Each NUMA instance publishes a DISTINCT mailbox on the shared host
        # /dev/shm; a client selects an instance by pointing at its shm path.
        env:
        - name: CERTUS_SHM_PATH
          value: "/dev/shm/certus-shmq-numa1"
        - name: CERTUS_NUMA_ID
          value: "1"
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
            memory: 1Gi
        securityContext:
          privileged: true
        volumeMounts:
        - name: config
          mountPath: /config
          readOnly: true
        - name: hugepages
          mountPath: /dev/hugepages
        - name: vfio
          mountPath: /dev/vfio
        - name: infiniband
          mountPath: /dev/infiniband
        - name: dev-shm
          mountPath: /dev/shm
        # Read-only, for the NUMA->GPU derivation above.
        - name: sysfs
          mountPath: /sys
          readOnly: true
      volumes:
      - name: config
        emptyDir: {}
      - name: sysfs
        hostPath:
          path: /sys
          type: Directory
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
# NOTE: no client-facing or peer Services. The shmq control transport is a
# node-local /dev/shm mailbox (one per NUMA instance:
# /dev/shm/certus-shmq-numa0 and -numa1), not a network endpoint — a client
# (vLLM) pod reaches an instance only by co-scheduling on the SAME node with
# `hostIPC: true`, the same `/dev/shm` hostPath mount, and the matching shm
# path. Cross-node server-to-server peer discovery (remote-lookup) is handled
# by zyre over the host network, so no peer Service is needed.
