/* The loader's checks and writes (loadcore.h) on the Mac, against buffers
   that hold the RAM as the camera has it. Run by test-load.sh.

     test-load BASE.VIEW OTHER.VIEW LOOK.PAT

   A view is the RAM from 0x53000000 to the end of the .data copy source, as
   rampatch.rs reads it (or RTOS.BIN of a dump). BASE is what LOOK.PAT was
   made against, OTHER another firmware. Each case starts from fresh
   buffers; after a failure the RAM must be as before. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "loadcore.h"

#define DATA_LMA 0x5407b540U
#define CODE_SIZE (CODE_END - RTOS_BASE)

static u8 *read_file(const char *path, u32 *size)
{
    FILE *f = fopen(path, "rb");
    if (!f) { perror(path); exit(2); }
    fseek(f, 0, SEEK_END);
    long n = ftell(f);
    fseek(f, 0, SEEK_SET);
    u8 *d = malloc((size_t)n);
    if (fread(d, 1, (size_t)n, f) != (size_t)n) { perror(path); exit(2); }
    fclose(f);
    *size = (u32)n;
    return d;
}

static u8 *base_view, *other_view;
static u8 code[CODE_SIZE], data[DATA_SIZE];
static u8 code0[CODE_SIZE], data0[DATA_SIZE];

/* fresh RAM from a view; code0/data0 keep it to compare with */
static void fill(const u8 *view)
{
    memcpy(code, view, CODE_SIZE);
    memcpy(data, view + (DATA_LMA - RTOS_BASE), DATA_SIZE);
    memcpy(code0, code, CODE_SIZE);
    memcpy(data0, data, DATA_SIZE);
}
static int unchanged(void)
{
    return !memcmp(code, code0, CODE_SIZE) && !memcmp(data, data0, DATA_SIZE);
}

/* the flush of the test: call number `fail_at` fails, call number
   `corrupt_at` changes the first byte it flushes (a write that did not
   land), call number `stray_at` changes a code byte outside every run */
static int calls, fail_at, corrupt_at, stray_at;
static int flush(volatile u8 *start, u32 n)
{
    (void)n;
    ++calls;
    if (calls == corrupt_at) start[0] ^= 0xff;
    if (calls == stray_at) code[0x100] ^= 0xff;
    return calls != fail_at;
}
static struct ram ram = { code, data, flush };

static int failures;
static void check(int ok, const char *what)
{
    printf("%s  %s\n", ok ? "ok  " : "FAIL", what);
    if (!ok) ++failures;
}

static u32 run_load(const u8 *patch, u32 size, struct load_result *r)
{
    calls = 0;
    return load(patch, size, &ram, r);
}

/* the first run of a kind, by its index in runs[] after a parse */
static int first_of(const u8 *patch, u32 kind)
{
    u32 o = 0x20, count = get32(patch + 8);
    for (u32 i = 0; i < count; ++i) {
        u32 n = get32(patch + o + 4), k = get32(patch + o + 8);
        if (k == kind) return (int)i;
        o += 12 + 2 * ((n + 3) & ~3U);
    }
    return -1;
}

/* the patch with the CRC at its end made right again */
static void fix_crc(u8 *p, u32 size)
{
    u32 c = crc_update(0xffffffffU, p, size - 4) ^ 0xffffffffU;
    p[size - 4] = (u8)c; p[size - 3] = (u8)(c >> 8); p[size - 2] = (u8)(c >> 16); p[size - 1] = (u8)(c >> 24);
}

int main(int argc, char **argv)
{
    if (argc != 4) { fprintf(stderr, "usage: test-load BASE.VIEW OTHER.VIEW LOOK.PAT\n"); return 2; }
    u32 base_size, other_size, size;
    base_view = read_file(argv[1], &base_size);
    other_view = read_file(argv[2], &other_size);
    u8 *patch = read_file(argv[3], &size);
    if (base_size < DATA_LMA - RTOS_BASE + DATA_SIZE || other_size < DATA_LMA - RTOS_BASE + DATA_SIZE) {
        fprintf(stderr, "views too short\n");
        return 2;
    }
    u8 *work = malloc(size);
    struct load_result r;
    u32 st;
    crc_init();

    /* 1. load */
    fill(base_view);
    st = run_load(patch, size, &r);
    printf("     %u runs, %u bytes, base %08x, patched %08x\n", r.runs, r.bytes, r.base_crc, r.patched_crc);
    check(st == 0 && !r.already, "loads");
    int all_after = 1;
    for (u32 i = 0; i < r.runs; ++i) all_after &= equal(runs[i].ram, runs[i].after, runs[i].n);
    check(all_after, "every run holds its bytes after");
    check(code_crc(&ram) == r.patched_crc, "code CRC is the patched one");
    /* nothing outside the runs changed */
    for (u32 i = 0; i < r.runs; ++i)
        for (u32 k = 0; k < runs[i].n; ++k) runs[i].ram[k] = runs[i].before[k];
    check(unchanged(), "nothing changed outside the runs");
    check(calls == (int)r.runs, "one flush per run");

    /* 2. a second run after the same power-on */
    fill(base_view);
    run_load(patch, size, &r);
    memcpy(code0, code, CODE_SIZE);
    memcpy(data0, data, DATA_SIZE);
    st = run_load(patch, size, &r);
    check(st == 0 && r.already && unchanged() && calls == 0, "second run: already loaded, no write");

    /* 3. the camera runs another firmware */
    fill(other_view);
    st = run_load(patch, size, &r);
    check(st == 5 && unchanged(), "other firmware: status 5, RAM as before");

    /* 4. damaged LOOK.PAT */
    fill(base_view);
    memcpy(work, patch, size);
    work[size / 2] ^= 1;
    st = run_load(work, size, &r);
    check(st == 4 && unchanged(), "flipped byte: status 4, RAM as before");
    st = run_load(patch, size - 8, &r);
    check(st == 4 && unchanged(), "truncated: status 4");
    memcpy(work, patch, size);
    work[0] = 'X';
    st = run_load(work, size, &r);
    check(st == 4 && unchanged(), "wrong magic: status 4");

    /* 5. a run outside code and .data */
    memcpy(work, patch, size);
    work[0x20 + 3] = 0x60;          /* first run's address -> 0x60xxxxxx */
    fix_crc(work, size);
    st = run_load(work, size, &r);
    check(st == 6 && unchanged(), "run outside: status 6, RAM as before");

    /* 6. bytes before of a .data run differ */
    int d = first_of(patch, KIND_DATA);
    if (d < 0) printf("skip  no .data run: cases 6 and 7 (bytes before differ, partly written)\n");
    if (d >= 0) {
        fill(base_view);
        parse(patch, size, &ram, &r);
        runs[d].ram[0] ^= 0x5a;
        memcpy(data0, data, DATA_SIZE);
        u32 addr = runs[d].address;
        st = run_load(patch, size, &r);
        check(st == 7 && r.detail == addr && unchanged(), "a run differs: status 7 at its address, RAM as before");

        /* 7. one run already written, the others not */
        fill(base_view);
        parse(patch, size, &ram, &r);
        for (u32 k = 0; k < runs[d].n; ++k) runs[d].ram[k] = runs[d].after[k];
        memcpy(data0, data, DATA_SIZE);
        st = run_load(patch, size, &r);
        check(st == 8 && r.detail == 1 && unchanged(), "partly written: status 8, RAM as before");
    }

    /* 8. a flush fails half way */
    fill(base_view);
    parse(patch, size, &ram, &r);
    fail_at = (int)r.runs / 2 + 1;
    st = run_load(patch, size, &r);
    fail_at = 0;
    check(st == 9 && unchanged(), "flush fails: status 9, rolled back");

    /* 9. the write of the last run does not land */
    u32 last = r.runs;
    fill(base_view);
    corrupt_at = (int)last;
    st = run_load(patch, size, &r);
    corrupt_at = 0;
    /* the roll-back writes the bytes before again, over the bad byte too */
    check(st == 10 && unchanged(), "write lost: status 10, rolled back");

    /* 10. something outside the runs changes while writing */
    fill(base_view);
    stray_at = (int)last;
    st = run_load(patch, size, &r);
    stray_at = 0;
    code[0x100] ^= 0xff;            /* the stray change itself is not ours to undo */
    check(st == 11 && unchanged(), "code CRC wrong afterwards: status 11, rolled back");

    printf(failures ? "%d FAILED\n" : "all passed\n", failures);
    return failures != 0;
}
