NVCC     ?= $(firstword $(wildcard /usr/local/cuda*/bin/nvcc))
CC       ?= gcc
OUTDIR   ?= /tmp

TARGETS  := $(OUTDIR)/cuda_bw_test $(OUTDIR)/cuda_d2d_test $(OUTDIR)/memcpy_bench

.PHONY: all clean

all: $(TARGETS)

$(OUTDIR)/cuda_bw_test: cuda_bw_test.cu
	$(NVCC) -O2 -o $@ $<

$(OUTDIR)/cuda_d2d_test: cuda_d2d_test.cu
	$(NVCC) -O2 -o $@ $<

$(OUTDIR)/memcpy_bench: memcpy_bench.c
	$(CC) -O3 -o $@ $< -lnuma

clean:
	rm -f $(TARGETS)
