// clang-format off
#include "vmlinux.h"
// clang-format on
#include <bpf/bpf_core_read.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>

#include "allocator.h"
#include "attach.h"
#include "event.h"
#include "process_tracking.bpf.h"
#include "rmap.bpf.h"
#include "rss.bpf.h"
#include "utils/event_helpers.h"
#include "utils/folio.h"
#include "utils/map_helpers.h"
#include "utils/mm_ownership.h"
#include "utils/process_tracking.h"

char LICENSE[] SEC("license") = "GPL";
