#!/bin/bash
#wget https://developer.download.nvidia.com/compute/cuda/13.2.1/local_installers/cuda-repo-rhel9-13-2-local-13.2.1_595.58.03-1.x86_64.rpm
scp emr:/tools/cuda-repo-rhel9-13-2-local-13.2.1_595.58.03-1.x86_64.rpm .
sudo rpm -i cuda-repo-rhel9-13-2-local-13.2.1_595.58.03-1.x86_64.rpm
sudo dnf clean all
sudo dnf -y install cuda-toolkit-13-2
