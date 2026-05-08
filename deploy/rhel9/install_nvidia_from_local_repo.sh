#!/bin/bash
scp emr:/tools/nvidia-driver-local-repo-rhel9-595.71.05-1.0-1.x86_64.rpm .
sudo rpm -i ./nvidia-driver-local-repo-rhel9-595.71.05-1.0-1.x86_64.rpm
sudo dnf clean all
sudo dnf -y module install nvidia-driver:latest-dkms
