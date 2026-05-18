#include <stddef.h>
#include <stdint.h>

#define DARWIN_PROT_READ 0x01
#define DARWIN_PROT_WRITE 0x02
#define DARWIN_MAP_PRIVATE 0x0002
#define DARWIN_MAP_ANON 0x1000
#define DARWIN_MAP_FAILED ((void *)-1)
#define DARWIN_PAGE_SIZE 0x4000

#define DARWIN_SYS_MPROTECT 74
#define DARWIN_SYS_MMAP 197

struct rootsqzSegment {
    uint64_t offset;
    uint64_t size;
    uint32_t prot;
    uint32_t reserved;
};

struct rootsqzImport {
    uintptr_t address;
    uint32_t weak;
    uint32_t reserved;
};

struct rootsqzFixup {
    uint64_t offset;
    uint64_t target;
    uint64_t addend;
    uint32_t import_index;
    uint32_t high8;
    uint32_t kind;
    uint32_t reserved;
};

extern const uint64_t rootsqz_image_size;
extern const uint64_t rootsqz_entry_offset;
extern const struct rootsqzSegment rootsqz_segments_start[];
extern const struct rootsqzSegment rootsqz_segments_end[];
extern const struct rootsqzImport rootsqz_imports_start[];
extern const struct rootsqzImport rootsqz_imports_end[];
extern const struct rootsqzFixup rootsqz_fixups_start[];
extern const struct rootsqzFixup rootsqz_fixups_end[];

static uint64_t darwin_syscall3(uint64_t number, uint64_t arg0, uint64_t arg1,
                                uint64_t arg2, uint64_t *err) {
    register uint64_t x0 __asm__("x0") = arg0;
    register uint64_t x1 __asm__("x1") = arg1;
    register uint64_t x2 __asm__("x2") = arg2;
    register uint64_t x16 __asm__("x16") = number;
    uint64_t failed;

    __asm__ volatile(
        "svc #0x80\n"
        "cset %w[failed], hs\n"
        : "+r"(x0), [failed] "=r"(failed)
        : "r"(x1), "r"(x2), "r"(x16)
        : "cc", "memory");
    *err = failed;
    return x0;
}

static uint64_t darwin_syscall6(uint64_t number, uint64_t arg0, uint64_t arg1,
                                uint64_t arg2, uint64_t arg3, uint64_t arg4,
                                uint64_t arg5, uint64_t *err) {
    register uint64_t x0 __asm__("x0") = arg0;
    register uint64_t x1 __asm__("x1") = arg1;
    register uint64_t x2 __asm__("x2") = arg2;
    register uint64_t x3 __asm__("x3") = arg3;
    register uint64_t x4 __asm__("x4") = arg4;
    register uint64_t x5 __asm__("x5") = arg5;
    register uint64_t x16 __asm__("x16") = number;
    uint64_t failed;

    __asm__ volatile(
        "svc #0x80\n"
        "cset %w[failed], hs\n"
        : "+r"(x0), [failed] "=r"(failed)
        : "r"(x1), "r"(x2), "r"(x3), "r"(x4), "r"(x5), "r"(x16)
        : "cc", "memory");
    *err = failed;
    return x0;
}

static void *sys_mmap(void *addr, size_t length, int prot, int flags, int fd,
                      uint64_t offset) {
    uint64_t err;
    uint64_t result = darwin_syscall6(DARWIN_SYS_MMAP, (uint64_t)addr,
                                      (uint64_t)length, (uint64_t)prot,
                                      (uint64_t)flags, (uint64_t)fd, offset,
                                      &err);
    return err ? DARWIN_MAP_FAILED : (void *)result;
}

static int sys_mprotect(void *addr, size_t length, int prot) {
    uint64_t err;
    darwin_syscall3(DARWIN_SYS_MPROTECT, (uint64_t)addr, (uint64_t)length,
                    (uint64_t)prot, &err);
    return err ? -1 : 0;
}

static uint64_t page_floor(uint64_t value, uint64_t page_size) {
    return value & ~(page_size - 1);
}

static uint64_t page_ceil(uint64_t value, uint64_t page_size) {
    return (value + page_size - 1) & ~(page_size - 1);
}

static void clear_instruction_cache(uint8_t *start, uint8_t *end) {
    uintptr_t begin = (uintptr_t)start;
    uintptr_t finish = (uintptr_t)end;

    __asm__ volatile("dsb ish" : : : "memory");

    for (uintptr_t p = begin & ~(uintptr_t)63; p < finish; p += 64) {
        __asm__ volatile("ic ivau, %0" : : "r"(p) : "memory");
    }
    __asm__ volatile("dsb ish\nisb" : : : "memory");
}

void *rootsqz_prepare_image(void) {
    void *image = sys_mmap(NULL, (size_t)rootsqz_image_size,
                           DARWIN_PROT_READ | DARWIN_PROT_WRITE,
                           DARWIN_MAP_PRIVATE | DARWIN_MAP_ANON, -1, 0);
    return image;
}

static uintptr_t resolve_import(uint32_t index) {
    size_t count = (size_t)(rootsqz_imports_end - rootsqz_imports_start);

    const struct rootsqzImport *import = &rootsqz_imports_start[index];
    return import->address;
}

static void apply_fixups(uint8_t *image) {
    for (const struct rootsqzFixup *fixup = rootsqz_fixups_start;
         fixup < rootsqz_fixups_end;
         fixup++) {
        uintptr_t *slot = (uintptr_t *)(image + fixup->offset);
        if (fixup->kind == 1) {
            *slot = resolve_import(fixup->import_index) + (uintptr_t)fixup->addend;
        } else {
            uintptr_t pointer = (uintptr_t)image + (uintptr_t)fixup->target;
            pointer |= (uintptr_t)fixup->high8 << 56;
            *slot = pointer;
        }
    }
}

static void protect_segments(uint8_t *image) {
    uint64_t page_size = DARWIN_PAGE_SIZE;
    for (const struct rootsqzSegment *segment = rootsqz_segments_start;
         segment < rootsqz_segments_end;
         segment++) {
        if (segment->size == 0) {
            continue;
        }

        uint64_t start = page_floor(segment->offset, page_size);
        uint64_t end = page_ceil(segment->offset + segment->size, page_size);
        sys_mprotect(image + start, (size_t)(end - start), (int)segment->prot);
    }
}

int rootsqz_launch_image(uint8_t *image, int argc, char **argv, char **envp) {
    apply_fixups(image);
    // We have to clear the instruction cache before jumping to the entry point, 
    // to make sure written code is visible to the instruction fetch unit.
    clear_instruction_cache(image, image + rootsqz_image_size);
    protect_segments(image);

    int (*entry)(int, char **, char **) =
        (int (*)(int, char **, char **))(void *)(image + rootsqz_entry_offset);
    return entry(argc, argv, envp);
}
