#!/bin/bash
sudo chmod -R a+rwx /dev/vfio
sudo chmod -R a+rwx /dev/hugepages
sudo sysctl -w net.core.rmem_max=67108864

# need to add to /etc/security/limits.conf
# * soft memlock unlimited
# * hard memlock unlimited
