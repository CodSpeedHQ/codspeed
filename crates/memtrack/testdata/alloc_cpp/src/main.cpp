#include <vector>
#include <thread>
#include <chrono>
#include <cstdint>
#include <cstdlib>

#ifdef USE_JEMALLOC
#include <jemalloc/jemalloc.h>
#endif

#ifdef USE_MIMALLOC
#include <mimalloc.h>
#endif

// USE_TCMALLOC: Uses gperftools libtcmalloc via dynamic linking
// No header include needed - tcmalloc overrides malloc/free symbols

// Prevent compiler from optimizing away allocations
// Similar to Rust's core::hint::black_box
template<typename T>
inline void black_box(T* ptr) {
    asm volatile("" : : "r,m"(ptr) : "memory");
}

int main() {
    std::this_thread::sleep_for(std::chrono::seconds(1));

    auto emit_marker = []() {
        uint8_t* ptr = new uint8_t[0xC0D59EED];
        black_box(ptr);
        delete[] ptr;
    };

    emit_marker();

    // array:
    uint32_t* allocated = new uint32_t[11111];
    black_box(allocated);
    delete[] allocated;

    // single element:
    uint64_t* var = new uint64_t;
    black_box(var);
    delete var;

    // vector:
    std::vector<uint32_t> vec(22222, 0);
    black_box(vec.data());

    // aligned allocation (64-byte alignment for cache line):
    uint8_t* aligned = static_cast<uint8_t*>(aligned_alloc(64, 64 * 512));
    black_box(aligned);
    free(aligned);

    emit_marker();
}
