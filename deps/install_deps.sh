#!/bin/bash
#
# Note: enable needed repos 
# ```sudo subscription-manager repos --enable codeready-builder-for-rhel-9-x86_64-rpms```
#
sudo dnf install -y fuse3-devel fuse3-libs numactl-libs numactl numactl-devel libuuid-devel libaio-devel ncurses-devel openssl-devel
sudo dnf install -y clang clang-devel glibc-headers glibc-devel gcc gcc-c++ make pkgconfig CUnit CUnit-devel
sudo dnf install -y protobuf-compiler python3-pip
pip install meson jinja2 pyelftools tabulate ninja
sudo ln -sf "$(python3 -c 'import shutil; print(shutil.which("meson"))')" /usr/bin/meson
sudo ln -sf "$(python3 -c 'import shutil; print(shutil.which("ninja"))')" /usr/bin/ninja
