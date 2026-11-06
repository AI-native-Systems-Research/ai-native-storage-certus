#!/bin/bash
#
# install-host-setup.sh - make a Certus worker node's host setup survive reboots.
#
# Run once per node, as root, from a checkout:
#   sudo deploy/k8s/host/install-host-setup.sh
#
# Installs:
#   /etc/modules-load.d/certus.conf      RDMA, vfio-pci and nvidia-peermem at boot
#                                        (only if absent: a site's own list is kept)
#   /etc/sysctl.d/90-certus.conf         net.core.rmem_max for the RDMA control plane
#   /usr/local/sbin/certus-host-setup    the per-boot script (certus-host-setup.sh)
#   certus-host-setup.service            enabled, and run now
#
# Memlock (/etc/security/limits.conf) and the kernel command line (IOMMU,
# hugepages) persist on their own; see deploy/k8s/README.md section 2.

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd -P)"

[[ ${EUID} -eq 0 ]] || { echo "must run as root" >&2; exit 1; }

if [[ -e /etc/modules-load.d/certus.conf ]]; then
    echo "keeping existing /etc/modules-load.d/certus.conf"
else
    cat > /etc/modules-load.d/certus.conf <<'EOF'
ib_core
ib_umad
ib_uverbs
mlx5_ib
vfio-pci
nvidia-peermem
EOF
    echo "wrote /etc/modules-load.d/certus.conf"
fi
# Load them now too, so this boot matches the next one. nvidia-peermem is
# absent on a node without an NVIDIA driver; that is not an error.
while read -r m; do
    [[ -n "${m}" && "${m}" != \#* ]] || continue
    modprobe "${m}" 2>/dev/null || echo "note: module ${m} did not load"
done < /etc/modules-load.d/certus.conf

echo "net.core.rmem_max = 67108864" > /etc/sysctl.d/90-certus.conf
sysctl -q -p /etc/sysctl.d/90-certus.conf

install -m 0755 "${HERE}/certus-host-setup.sh" /usr/local/sbin/certus-host-setup
install -m 0644 "${HERE}/certus-host-setup.service" /etc/systemd/system/certus-host-setup.service
systemctl daemon-reload
systemctl enable certus-host-setup.service
# restart, not start: on a re-install, run the new script even though the
# unit is already active (RemainAfterExit).
systemctl restart certus-host-setup.service || true
journalctl -b -u certus-host-setup.service --no-pager -o cat | tail -n 20
systemctl is-active --quiet certus-host-setup.service ||
    { echo "certus-host-setup.service failed; see journalctl -u certus-host-setup" >&2; exit 1; }
