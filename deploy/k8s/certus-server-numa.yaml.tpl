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
          exec certus-server-yaml $ARGS --listen 0.0.0.0:${CERTUS_PORT} --memory-tier-size 4G
        env:
        - name: CERTUS_PORT
          value: "50051"
        - name: CERTUS_NUMA_ID
          value: "0"
        ports:
        - containerPort: 50051
          name: grpc
        - containerPort: 50053
          name: peer
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
          exec certus-server-yaml $ARGS --listen 0.0.0.0:${CERTUS_PORT} --memory-tier-size 4G
        env:
        - name: CERTUS_PORT
          value: "50052"
        - name: CERTUS_NUMA_ID
          value: "1"
        ports:
        - containerPort: 50052
          name: grpc
        - containerPort: 50054
          name: peer
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
      nodeSelector:
        certus.ai/worker: "true"
---
# Client service for NUMA 0 instances
apiVersion: v1
kind: Service
metadata:
  name: certus-server-numa0
  namespace: certus
  labels:
    app: certus-server
    app.kubernetes.io/instance: numa0
spec:
  selector:
    app: certus-server
    app.kubernetes.io/instance: numa0
  ports:
  - port: 50051
    targetPort: 50051
    name: grpc
    protocol: TCP
  internalTrafficPolicy: Local
---
# Client service for NUMA 1 instances
apiVersion: v1
kind: Service
metadata:
  name: certus-server-numa1
  namespace: certus
  labels:
    app: certus-server
    app.kubernetes.io/instance: numa1
spec:
  selector:
    app: certus-server
    app.kubernetes.io/instance: numa1
  ports:
  - port: 50052
    targetPort: 50052
    name: grpc
    protocol: TCP
  internalTrafficPolicy: Local
---
# Headless service for server-to-server peer discovery (all instances)
# Servers resolve certus-servers.certus.svc.cluster.local
apiVersion: v1
kind: Service
metadata:
  name: certus-servers
  namespace: certus
  labels:
    app: certus-server
spec:
  clusterIP: None
  selector:
    app: certus-server
  ports:
  - port: 50053
    targetPort: 50053
    name: peer-numa0
    protocol: TCP
  - port: 50054
    targetPort: 50054
    name: peer-numa1
    protocol: TCP
