# Run with `python3 -X perf`: the perf trampoline gives `allocation` a
# `py::allocation:<file>` entry in /tmp/perf-<pid>.map, so the native malloc
# below can be attributed to this Python function offline.
import ctypes

libc = ctypes.CDLL(None)
libc.malloc.restype = ctypes.c_void_p
libc.malloc.argtypes = [ctypes.c_size_t]
libc.free.argtypes = [ctypes.c_void_p]

ALLOCATION_SIZE = 2_000_001


def allocation():
    ptr = libc.malloc(ALLOCATION_SIZE)
    libc.free(ptr)


allocation()
