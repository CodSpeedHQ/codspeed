#ifndef __FOLIO_H__
#define __FOLIO_H__

#define FOLIO_MAPPING_ANON 0x1UL

const volatile __u32 page_shift = 12;

/* Kernels < 6.18 store folio->flags as a bare unsigned long instead of
 * memdesc_flags_t; probe which layout the running kernel has. */
struct folio___legacy {
    unsigned long flags;
} __attribute__((preserve_access_index));

static __always_inline unsigned long folio_read_flags(struct folio* folio) {
    if (bpf_core_field_exists(folio->flags.f)) {
        return BPF_CORE_READ(folio, flags).f;
    }
    return BPF_CORE_READ((struct folio___legacy*)folio, flags);
}

/* Kernels < 6.6 store the large-folio order in a dedicated byte instead of
 * the low byte of _flags_1. */
struct folio___order_byte {
    unsigned char _folio_order;
} __attribute__((preserve_access_index));

static __always_inline unsigned long folio_order(struct folio* folio) {
    if (bpf_core_field_exists(((struct folio___order_byte*)folio)->_folio_order)) {
        return BPF_CORE_READ((struct folio___order_byte*)folio, _folio_order);
    }
    return BPF_CORE_READ(folio, _flags_1) & 0xff;
}

static __always_inline __u64 folio_nr_pages_est(struct folio* folio) {
    unsigned long flags = folio_read_flags(folio);
    if (!(flags & (1UL << bpf_core_enum_value(enum pageflags, PG_head)))) {
        return 1;
    }
    unsigned long order = folio_order(folio);
    return 1UL << order;
}

static __always_inline int folio_is_anon(struct folio* folio) {
    unsigned long mapping = (unsigned long)BPF_CORE_READ(folio, mapping);
    return (mapping & FOLIO_MAPPING_ANON) != 0;
}

/* Mirrors the kernel's mm_counter(): anon folios are also swapbacked, so the
 * anon check must come first. */
static __always_inline __s32 folio_mm_counter(struct folio* folio) {
    if (folio_is_anon(folio)) {
        return MM_ANONPAGES;
    }
    unsigned long flags = folio_read_flags(folio);
    if (flags & (1UL << bpf_core_enum_value(enum pageflags, PG_swapbacked))) {
        return MM_SHMEMPAGES;
    }
    return MM_FILEPAGES;
}

static __always_inline __u64 folio_page_address(struct folio* folio, struct page* page,
                                                struct vm_area_struct* vma) {
    __u64 page_idx = ((__u64)page - (__u64)folio) / bpf_core_type_size(struct page);
    __u64 pgoff = BPF_CORE_READ(folio, index) + page_idx;
    __u64 vm_pgoff = BPF_CORE_READ(vma, vm_pgoff);
    __u64 vm_start = BPF_CORE_READ(vma, vm_start);
    if (pgoff < vm_pgoff) {
        return vm_start;
    }
    return vm_start + ((pgoff - vm_pgoff) << page_shift);
}

#endif /* __FOLIO_H__ */
