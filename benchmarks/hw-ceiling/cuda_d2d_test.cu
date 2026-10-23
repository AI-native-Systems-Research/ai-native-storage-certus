#include <stdio.h>
#include <cuda_runtime.h>
#include <stdlib.h>

#define CHECK(call) do { \
    cudaError_t e = (call); \
    if (e != cudaSuccess) { fprintf(stderr, "CUDA error %s:%d: %s\n", __FILE__, __LINE__, cudaGetErrorString(e)); exit(1); } \
} while(0)

static double measure_d2d_nstream(void **src, void **dst, size_t sz, int nstreams, int warmup, int iters) {
    cudaStream_t streams[4];
    cudaEvent_t start, stop;
    for (int s = 0; s < nstreams; s++) CHECK(cudaStreamCreate(&streams[s]));
    CHECK(cudaEventCreate(&start));
    CHECK(cudaEventCreate(&stop));
    for (int i = 0; i < warmup; i++)
        for (int s = 0; s < nstreams; s++)
            CHECK(cudaMemcpyAsync(dst[s], src[s], sz, cudaMemcpyDeviceToDevice, streams[s]));
    CHECK(cudaDeviceSynchronize());
    CHECK(cudaEventRecord(start));
    for (int i = 0; i < iters; i++)
        for (int s = 0; s < nstreams; s++)
            CHECK(cudaMemcpyAsync(dst[s], src[s], sz, cudaMemcpyDeviceToDevice, streams[s]));
    for (int s = 0; s < nstreams; s++) CHECK(cudaStreamSynchronize(streams[s]));
    CHECK(cudaEventRecord(stop));
    CHECK(cudaEventSynchronize(stop));
    float ms = 0;
    CHECK(cudaEventElapsedTime(&ms, start, stop));
    for (int s = 0; s < nstreams; s++) CHECK(cudaStreamDestroy(streams[s]));
    cudaEventDestroy(start);
    cudaEventDestroy(stop);
    return (double)sz * nstreams * iters / (ms / 1000.0) / 1e9;
}

int main() {
    int ngpu;
    CHECK(cudaGetDeviceCount(&ngpu));

    size_t sizes[] = {128*1024, 512*1024, 1*1024*1024, 2*1024*1024, 4*1024*1024,
                      8*1024*1024, 16*1024*1024};
    const char *size_names[] = {"128K","512K","1M","2M","4M","8M","16M"};
    int nsizes = 7;
    int warmup = 10, iters = 100;

    for (int g = 0; g < ngpu; g++) {
        CHECK(cudaSetDevice(g));
        cudaDeviceProp prop;
        CHECK(cudaGetDeviceProperties(&prop, g));
        printf("=== GPU %d: %s D2D ===\n", g, prop.name);
        printf("%-8s  %14s  %14s  %14s\n", "Size", "1-stream(GB/s)", "2-stream(GB/s)", "4-stream(GB/s)");

        size_t maxsz = sizes[nsizes-1];
        void *src[4], *dst[4];
        for (int s = 0; s < 4; s++) {
            CHECK(cudaMalloc(&src[s], maxsz));
            CHECK(cudaMalloc(&dst[s], maxsz));
            CHECK(cudaMemset(src[s], 0xAB, maxsz));
            CHECK(cudaMemset(dst[s], 0x00, maxsz));
        }

        for (int i = 0; i < nsizes; i++) {
            size_t sz = sizes[i];
            double d1 = measure_d2d_nstream(src, dst, sz, 1, warmup, iters);
            double d2 = measure_d2d_nstream(src, dst, sz, 2, warmup, iters);
            double d4 = measure_d2d_nstream(src, dst, sz, 4, warmup, iters);
            printf("%-8s  %14.3f  %14.3f  %14.3f\n", size_names[i], d1, d2, d4);
        }

        for (int s = 0; s < 4; s++) {
            cudaFree(src[s]);
            cudaFree(dst[s]);
        }
        printf("\n");
    }
    return 0;
}
