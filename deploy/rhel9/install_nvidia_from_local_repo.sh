#!/bin/bash
scp emr:/tools/nvidia-driver-local-repo-rhel9-595.71.05-1.0-1.x86_64.rpm .
rpm -i ./nvidia-driver-local-repo-rhel9-595.71.05-1.0-1.x86_64.rpm
dnf clean all
dnf -y module install nvidia-driver:latest-dkms
