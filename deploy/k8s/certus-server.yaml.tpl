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
      containers:
      - name: certus
        image: %%CERTUS_REGISTRY%%/%%CERTUS_REPO%%/%%CERTUS_IMAGE%%:%%CERTUS_TAG%%
        args:
        - "--drive-count"
        - "1"
        - "--listen"
        - "0.0.0.0:50051"
        - "--memory-tier-size"
        - "4G"
        ports:
        - containerPort: 50051
          hostPort: 50051
          name: grpc
        - containerPort: 50052
          hostPort: 50052
          name: peer
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
      nodeSelector:
        certus.ai/worker: "true"
---
# Client-facing service: routes only to the server on the same node
apiVersion: v1
kind: Service
metadata:
  name: certus-server
  namespace: certus
  labels:
    app: certus-server
spec:
  selector:
    app: certus-server
  ports:
  - port: 50051
    targetPort: 50051
    name: grpc
    protocol: TCP
  internalTrafficPolicy: Local
---
# Headless service for server-to-server peer discovery
# Servers resolve certus-servers.certus.svc.cluster.local
# to get all peer pod IPs for RDMA connection exchange
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
  - port: 50052
    targetPort: 50052
    name: peer
    protocol: TCP
