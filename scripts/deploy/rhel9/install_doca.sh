#!/bin/bash
#
# https://docs.nvidia.com/dgx/dgx-el9-user-guide/installing-dofed-steps.html
#
sudo wget http://www.mellanox.com/downloads/ofed/RPM-GPG-KEY-Mellanox-SHA256
sudo rpm --import RPM-GPG-KEY-Mellanox-SHA256
sudo rpm -q gpg-pubkey --qf '%{NAME}-%{VERSION}-%{RELEASE}\t%{SUMMARY}\n' | grep Mellanox
sudo echo "[doca]
name=DOCA Online Repo
baseurl=https://linux.mellanox.com/public/repo/doca/DGX_latest_DOCA/rhel9/x86_64/
enabled=1
gpgcheck=0" > /tmp/doca.repo
sudo mv /tmp/doca.repo /etc/yum.repos.d/doca.repo
sudo chown root.root /etc/yum.repos.d/doca.repo
sudo dnf clean all -y
sudo dnf update --nobest
sudo dnf makecache -y
sudo dnf install -y kernel-modules-extra-$(uname -r)
sudo dnf install -y doca-ofed
sudo dnf install -y nvidia-mlnx-config
# sudo dnf install mlnx-fw-updater
sudo dracut -f
# sudo dnf install mlnxofed-docs
