Model: Claude Opus 4.6

## Add spec-kit-sync

specify extension add spec-kit-sync --from https://github.com/bgervin/spec-kit-sync/archive/refs/heads/master.zip

## Constitution

/speckit.constitution Create principles focused on code quality, extensive testing, 
established good engineering practice, maintainability and meeting performance requirements.  All code must run on the Linux operating system and support RHEL 9 and RHEL 10.  Kernel-level code should be written in C, and support kernel version 5.14 and beyond.  User-level code should be written in Rust.  Avoid Rust unsafe code where possible.  All Rust performance tests should be based on Criterion and must be available for all performance sensitive code.  Assurance of code correctness is of high importance.  

## Features

/speckit.specify Write a kernel module that allows a Rust user-space library to access the NVIDIA driver kernel functions nvidia_p2p_get_pages_persistent() so that GPU memory can be pinned.  The user API should take a virtual address and length, and get back a physical address for the pinned memory.  An API to "unpin" previously pinned memory should also be included. The purpose of pinning GPU memory is to DMA transfer data directly from SSD into the GPU, and vice-versa.  You can use the header "/usr/src/nvidia-580.126.20/nvidia-peermem/nv-p2p.h". The location of this header should be automatically discovered by the build.

/speckit-specify Add a test in Rust, that uses the cudaMalloc API to allocate memory that can be pinned by the driver.                                          

+ Add a README.md in nvidia_p2p dir, that describes how to build kernel module, load it, and run the cuda_pin_test and pin_upin benchmark                         
