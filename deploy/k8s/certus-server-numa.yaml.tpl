---
apiVersion: v1
kind: Namespace
metadata:
  name: certus
---
# NUMA 0 DaemonSet (cluster-wide)
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: certus-server-numa0
  namespace: certus
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
          exec certus-server-yaml $ARGS --shm-path ${CERTUS_SHM_PATH} --channels 32 --memory-tier-size 4G
        # Each NUMA instance publishes a DISTINCT mailbox on the shared host
        # /dev/shm; a client selects an instance by pointing at its shm path.
        env:
        - name: CERTUS_SHM_PATH
          value: "/dev/shm/certus-shmq-numa0"
        - name: CERTUS_NUMA_ID
          value: "0"
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
        certus.ai/worker: "true"
---
# NUMA 1 DaemonSet (cluster-wide)
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: certus-server-numa1
  namespace: certus
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
          exec certus-server-yaml $ARGS --shm-path ${CERTUS_SHM_PATH} --channels 32 --memory-tier-size 4G
        # Each NUMA instance publishes a DISTINCT mailbox on the shared host
        # /dev/shm; a client selects an instance by pointing at its shm path.
        env:
        - name: CERTUS_SHM_PATH
          value: "/dev/shm/certus-shmq-numa1"
        - name: CERTUS_NUMA_ID
          value: "1"
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
        certus.ai/worker: "true"
# NOTE: no client-facing or peer Services. The shmq control transport is a
# node-local /dev/shm mailbox (one per NUMA instance:
# /dev/shm/certus-shmq-numa0 and -numa1), not a network endpoint — a client
# (vLLM) pod reaches an instance only by co-scheduling on the SAME node with
# `hostIPC: true`, the same `/dev/shm` hostPath mount, and the matching shm
# path. Cross-node server-to-server peer discovery (remote-lookup) is handled
# by zyre over the host network, so no peer Service is needed.
