#!/bin/bash
# Note: you may need `dnf module reset nvidia-driver` to clear up from earlier versions
#
# wget https://us.download.nvidia.com/tesla/595.71.05/nvidia-driver-local-repo-rhel9-595.71.05-1.0-1.x86_64.rpm
# scp emr:/tools/nvidia-driver-local-repo-rhel9-595.71.05-1.0-1.x86_64.rpm .
#
sudo rpm -i ./nvidia-driver-local-repo-rhel9-595.71.05-1.0-1.x86_64.rpm
sudo dnf clean all
sudo dnf -y module install nvidia-driver:latest-dkms
