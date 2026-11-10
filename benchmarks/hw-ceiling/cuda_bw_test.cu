#include <stdio.h>
#include <cuda_runtime.h>
#include <stdlib.h>

#define CHECK(call) do { \
    cudaError_t e = (call); \
    if (e != cudaSuccess) { fprintf(stderr, "CUDA error %s:%d: %s\n", __FILE__, __LINE__, cudaGetErrorString(e)); exit(1); } \
} while(0)

static double measure_h2d_1stream(void *h_buf, void *d_buf, size_t sz, int warmup, int iters) {
    cudaEvent_t start, stop;
    CHECK(cudaEventCreate(&start));
    CHECK(cudaEventCreate(&stop));
    for (int i = 0; i < warmup; i++) CHECK(cudaMemcpy(d_buf, h_buf, sz, cudaMemcpyHostToDevice));
    CHECK(cudaEventRecord(start));
    for (int i = 0; i < iters; i++) CHECK(cudaMemcpy(d_buf, h_buf, sz, cudaMemcpyHostToDevice));
    CHECK(cudaEventRecord(stop));
    CHECK(cudaEventSynchronize(stop));
    float ms = 0;
    CHECK(cudaEventElapsedTime(&ms, start, stop));
    cudaEventDestroy(start);
    cudaEventDestroy(stop);
    return (double)sz * iters / (ms / 1000.0) / 1e9;
}

static double measure_d2h_1stream(void *h_buf, void *d_buf, size_t sz, int warmup, int iters) {
    cudaEvent_t start, stop;
    CHECK(cudaEventCreate(&start));
    CHECK(cudaEventCreate(&stop));
    for (int i = 0; i < warmup; i++) CHECK(cudaMemcpy(h_buf, d_buf, sz, cudaMemcpyDeviceToHost));
    CHECK(cudaEventRecord(start));
    for (int i = 0; i < iters; i++) CHECK(cudaMemcpy(h_buf, d_buf, sz, cudaMemcpyDeviceToHost));
    CHECK(cudaEventRecord(stop));
    CHECK(cudaEventSynchronize(stop));
    float ms = 0;
    CHECK(cudaEventElapsedTime(&ms, start, stop));
    cudaEventDestroy(start);
    cudaEventDestroy(stop);
    return (double)sz * iters / (ms / 1000.0) / 1e9;
}

static double measure_h2d_nstream(void **h_bufs, void **d_bufs, size_t sz, int nstreams, int warmup, int iters) {
    cudaStream_t streams[8];
    cudaEvent_t start, stop;
    for (int s = 0; s < nstreams; s++) CHECK(cudaStreamCreate(&streams[s]));
    CHECK(cudaEventCreate(&start));
    CHECK(cudaEventCreate(&stop));
    for (int i = 0; i < warmup; i++)
        for (int s = 0; s < nstreams; s++)
            CHECK(cudaMemcpyAsync(d_bufs[s], h_bufs[s], sz, cudaMemcpyHostToDevice, streams[s]));
    CHECK(cudaDeviceSynchronize());
    CHECK(cudaEventRecord(start, streams[0]));
    for (int i = 0; i < iters; i++)
        for (int s = 0; s < nstreams; s++)
            CHECK(cudaMemcpyAsync(d_bufs[s], h_bufs[s], sz, cudaMemcpyHostToDevice, streams[s]));
    for (int s = 0; s < nstreams; s++) CHECK(cudaStreamSynchronize(streams[s]));
    CHECK(cudaEventRecord(stop));
    CHECK(cudaEventSynchronize(stop));
    float ms = 0;
    CHECK(cudaEventElapsedTime(&ms, start, stop));
    for (int s = 0; s < nstreams; s++) CHECK(cudaStreamDestroy(streams[s]));
    cudaEventDestroy(start);
    cudaEventDestroy(stop);
    // total bytes = sz * nstreams * iters
    return (double)sz * nstreams * iters / (ms / 1000.0) / 1e9;
}

int main() {
    int ngpu;
    CHECK(cudaGetDeviceCount(&ngpu));
    printf("GPUs detected: %d\n\n", ngpu);

    size_t sizes[] = {128*1024, 512*1024, 1*1024*1024, 2*1024*1024, 4*1024*1024,
                      8*1024*1024, 16*1024*1024, 32*1024*1024, 64*1024*1024, 256*1024*1024};
    const char *size_names[] = {"128K","512K","1M","2M","4M","8M","16M","32M","64M","256M"};
    int nsizes = 10;
    int warmup = 5, iters = 50;

    for (int g = 0; g < ngpu; g++) {
        CHECK(cudaSetDevice(g));
        cudaDeviceProp prop;
        CHECK(cudaGetDeviceProperties(&prop, g));
        printf("=== GPU %d: %s ===\n", g, prop.name);
        printf("%-8s  %12s  %12s  %12s  %12s  %12s\n",
               "Size", "H2D-1s(GB/s)", "H2D-2s(GB/s)", "H2D-4s(GB/s)", "H2D-8s(GB/s)", "D2H-1s(GB/s)");

        // allocate max size buffers
        size_t maxsz = sizes[nsizes-1];
        void *h_buf[8], *d_buf[8];
        for (int s = 0; s < 8; s++) {
            CHECK(cudaMallocHost(&h_buf[s], maxsz));
            CHECK(cudaMalloc(&d_buf[s], maxsz));
            memset(h_buf[s], 0xAB, maxsz);
        }

        for (int i = 0; i < nsizes; i++) {
            size_t sz = sizes[i];
            double h2d1 = measure_h2d_1stream(h_buf[0], d_buf[0], sz, warmup, iters);
            double h2d2 = measure_h2d_nstream(h_buf, d_buf, sz, 2, warmup, iters);
            double h2d4 = measure_h2d_nstream(h_buf, d_buf, sz, 4, warmup, iters);
            double h2d8 = measure_h2d_nstream(h_buf, d_buf, sz, 8, warmup, iters);
            double d2h1 = measure_d2h_1stream(h_buf[0], d_buf[0], sz, warmup, iters);
            printf("%-8s  %12.3f  %12.3f  %12.3f  %12.3f  %12.3f\n",
                   size_names[i], h2d1, h2d2, h2d4, h2d8, d2h1);
        }

        for (int s = 0; s < 8; s++) {
            cudaFreeHost(h_buf[s]);
            cudaFree(d_buf[s]);
        }
        printf("\n");
    }
    return 0;
}
