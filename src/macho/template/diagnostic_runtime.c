#include <dlfcn.h>
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

struct WebsqzSegment {
    uint64_t offset;
    uint64_t size;
    uint32_t prot;
    uint32_t reserved;
};

struct WebsqzImport {
    const char *name;
    uint32_t weak;
    uint32_t reserved;
};

struct WebsqzFixup {
    uint64_t offset;
    uint64_t target;
    uint64_t addend;
    uint32_t import_index;
    uint32_t high8;
    uint32_t kind;
    uint32_t reserved;
};

extern const uint64_t websqz_image_size;
extern const uint64_t websqz_entry_offset;
extern const struct WebsqzSegment websqz_segments_start[];
extern const struct WebsqzSegment websqz_segments_end[];
extern const struct WebsqzImport websqz_imports_start[];
extern const struct WebsqzImport websqz_imports_end[];
extern const struct WebsqzFixup websqz_fixups_start[];
extern const struct WebsqzFixup websqz_fixups_end[];

static uint64_t page_floor(uint64_t value, uint64_t page_size) {
    return value & ~(page_size - 1);
}

static uint64_t page_ceil(uint64_t value, uint64_t page_size) {
    return (value + page_size - 1) & ~(page_size - 1);
}

void *websqz_prepare_image(void) {
    void *image = mmap(NULL, (size_t)websqz_image_size, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANON, -1, 0);
    if (image == MAP_FAILED) {
        fprintf(stderr, "websqz: mmap failed: %s\n", strerror(errno));
        exit(1);
    }
    return image;
}

static uintptr_t resolve_import(uint32_t index) {
    size_t count = (size_t)(websqz_imports_end - websqz_imports_start);
    if (index >= count) {
        fprintf(stderr, "websqz: invalid import index %u\n", index);
        exit(1);
    }

    const struct WebsqzImport *import = &websqz_imports_start[index];
    const char *name = import->name;
    if (name[0] == '_') {
        name++;
    }

    void *symbol = dlsym(RTLD_DEFAULT, name);
    if (!symbol && !import->weak) {
        fprintf(stderr, "websqz: dlsym(%s) failed: %s\n", name, dlerror());
        exit(1);
    }
    return (uintptr_t)symbol;
}

static void apply_fixups(uint8_t *image) {
    for (const struct WebsqzFixup *fixup = websqz_fixups_start;
         fixup < websqz_fixups_end;
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
    uint64_t page_size = (uint64_t)getpagesize();
    for (const struct WebsqzSegment *segment = websqz_segments_start;
         segment < websqz_segments_end;
         segment++) {
        if (segment->size == 0) {
            continue;
        }

        uint64_t start = page_floor(segment->offset, page_size);
        uint64_t end = page_ceil(segment->offset + segment->size, page_size);
        if (mprotect(image + start, (size_t)(end - start), (int)segment->prot) != 0) {
            fprintf(stderr, "websqz: mprotect failed: %s\n", strerror(errno));
            exit(1);
        }
    }
}

int websqz_launch_image(uint8_t *image, int argc, char **argv, char **envp) {
    apply_fixups(image);
    __builtin___clear_cache((char *)image, (char *)image + websqz_image_size);
    protect_segments(image);

    int (*entry)(int, char **, char **) =
        (int (*)(int, char **, char **))(void *)(image + websqz_entry_offset);
    return entry(argc, argv, envp);
}
