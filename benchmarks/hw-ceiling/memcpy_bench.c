#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <numa.h>
#include <numaif.h>

static double now_sec() {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec + ts.tv_nsec * 1e-9;
}

static double bench_memcpy(void *dst, void *src, size_t sz, int iters) {
    // warmup
    for (int i = 0; i < 3; i++) memcpy(dst, src, sz);
    double t0 = now_sec();
    for (int i = 0; i < iters; i++) memcpy(dst, src, sz);
    double t1 = now_sec();
    return (double)sz * iters / (t1 - t0) / 1e9;
}

int main() {
    int nnodes = numa_available() >= 0 ? numa_num_configured_nodes() : 1;
    printf("NUMA nodes available: %d\n\n", nnodes);

    size_t sizes[] = {4*1024*1024, 16*1024*1024, 64*1024*1024, 256*1024*1024};
    const char *names[] = {"4M","16M","64M","256M"};
    int nsizes = 4;

    printf("%-8s  %16s  %16s\n", "Size", "Same-NUMA(GB/s)", "Cross-NUMA(GB/s)");

    for (int i = 0; i < nsizes; i++) {
        size_t sz = sizes[i];
        int iters = (sz <= 16*1024*1024) ? 200 : 50;

        // Same-NUMA: alloc both on node 0
        void *src0 = numa_alloc_onnode(sz, 0);
        void *dst0 = numa_alloc_onnode(sz, 0);
        memset(src0, 0xAB, sz);
        memset(dst0, 0, sz);
        double same = bench_memcpy(dst0, src0, sz, iters);

        double cross = -1.0;
        if (nnodes >= 2) {
            void *dst1 = numa_alloc_onnode(sz, 1);
            memset(dst1, 0, sz);
            cross = bench_memcpy(dst1, src0, sz, iters);
            numa_free(dst1, sz);
        }

        if (cross >= 0)
            printf("%-8s  %16.3f  %16.3f\n", names[i], same, cross);
        else
            printf("%-8s  %16.3f  %16s\n", names[i], same, "N/A");

        numa_free(src0, sz);
        numa_free(dst0, sz);
    }
    return 0;
}
