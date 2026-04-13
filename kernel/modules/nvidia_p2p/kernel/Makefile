# Makefile for nvidia_p2p_pin kernel module
#
# Auto-discovers the NVIDIA driver source directory containing nv-p2p.h.
# Requires: NVIDIA proprietary driver installed with development headers.

KVER ?= $(shell uname -r)
KDIR ?= /lib/modules/$(KVER)/build

# Auto-discover nv-p2p.h location: find the newest NVIDIA driver source dir
NVIDIA_SRC := $(shell ls -d /usr/src/nvidia-*/nvidia 2>/dev/null | sort -V | tail -1)

ifeq ($(NVIDIA_SRC),)
$(error NVIDIA driver source not found. Install nvidia-driver-devel or check /usr/src/nvidia-*/nvidia/nv-p2p.h)
endif

# Verify nv-p2p.h exists at discovered path
ifeq ($(wildcard $(NVIDIA_SRC)/nv-p2p.h),)
$(error nv-p2p.h not found at $(NVIDIA_SRC)/nv-p2p.h)
endif

# Auto-discover NVIDIA driver version from source dir name
NVIDIA_VER := $(shell echo $(NVIDIA_SRC) | sed 's|.*/nvidia-\([^/]*\)/nvidia|\1|')

# Auto-discover NVIDIA Module.symvers from DKMS build
NVIDIA_SYMVERS := $(shell find /var/lib/dkms/nvidia/$(NVIDIA_VER)/$(KVER) \
	-name "Module.symvers" 2>/dev/null | head -1)

ifeq ($(NVIDIA_SYMVERS),)
$(warning NVIDIA Module.symvers not found in DKMS. modpost may report undefined symbols.)
endif

export NVIDIA_SRC
export KBUILD_EXTRA_SYMBOLS := $(NVIDIA_SYMVERS)

.PHONY: modules clean

modules:
	@echo "NVIDIA source: $(NVIDIA_SRC)"
	@echo "NVIDIA symvers: $(NVIDIA_SYMVERS)"
	$(MAKE) -C $(KDIR) M=$(CURDIR) modules

clean:
	$(MAKE) -C $(KDIR) M=$(CURDIR) clean
