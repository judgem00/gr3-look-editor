/* RAM loader for the GR III (phase 2 of docs/plan-ram-loader.md): writes a
   look, exported by the editor as LOOK.PAT (core/src/rampatch.rs), into the
   memory of the running RTOS. Nothing is flashed; after power-off the camera
   runs its firmware unchanged.

   Runs in the camera's Linux, started by SCRIPT/STARTUP.TTL through `lcmd`.
   The checks and writes are in loadcore.h (tested on the Mac by
   test-load.c); this file reads LOOK.PAT, maps the RAM and writes the result
   files. The way it writes follows grkit's installer (github.com/ahonn/grkit,
   runtime/installer.c, MIT, Copyright (c) ahonn): /dev/mem with O_SYNC,
   then `dsb` and the cacheflush syscall over each written range.

   Output, in the folder of this program:
     LOAD.OK    runs, bytes, CRCs and "already" 0/1, after a complete load
     LOAD.ERR   status and detail (loadcore.h); the RAM is as before
   Status here besides those of loadcore.h: 1 /dev/mem, 2 mmap, 3 LOOK.PAT
   missing or too large.
   LOAD.LCK admits one run at a time; STARTUP.TTL deletes it first. */
#include "common.h"
#include "loadcore.h"

#define CODE_MAP_END 0x5407c000U
#define DATA_MAP_END 0x5503b000U
#define PATCH_MAX (1U << 20)

static u8 patch[PATCH_MAX];

static void fail(u32 code, u32 detail)
{
    used = 0;
    line("error", code);
    line("detail", detail);
    save(DIR "LOAD.ERR");
    sys(SYS_EXIT, code, 0, 0, 0, 0, 0);
    for (;;) {}
}

static int flush(volatile u8 *start, u32 n)
{
    __asm__ volatile("dsb sy" ::: "memory");
    return sys(SYS_CACHEFLUSH, (u32)start, (u32)(start + n), 0, 0, 0, 0) == 0;
}

static volatile u8 *map(u32 fd, u32 start, u32 end)
{
    long m = sys(SYS_MMAP2, 0, end - start, PROT_READ | PROT_WRITE, MAP_SHARED, fd, start >> 12);
    if ((u32)m >= 0xfffff001U) fail(2, (u32)m);
    return (volatile u8 *)m;
}

__attribute__((used)) static void start(void)
{
    /* a second run while the first one writes stops here */
    long lock = create(DIR "LOAD.LCK", O_EXCL);
    if (lock < 0) sys(SYS_EXIT, 0, 0, 0, 0, 0, 0);
    sys(SYS_CLOSE, (u32)lock, 0, 0, 0, 0, 0);

    long in = sys(SYS_OPEN, (u32)(DIR "LOOK.PAT"), O_RDONLY, 0, 0, 0, 0);
    if (in < 0) fail(3, (u32)in);
    u32 size = 0;
    for (;;) {
        if (size == PATCH_MAX) fail(3, PATCH_MAX);
        long r = sys(SYS_READ, (u32)in, (u32)(patch + size), PATCH_MAX - size, 0, 0, 0);
        if (r < 0) fail(3, (u32)r);
        if (r == 0) break;
        size += (u32)r;
    }
    sys(SYS_CLOSE, (u32)in, 0, 0, 0, 0, 0);

    long fd = sys(SYS_OPEN, (u32)"/dev/mem", O_RDWR | O_SYNC, 0, 0, 0, 0);
    if (fd < 0) fail(1, (u32)fd);
    struct ram m = { map((u32)fd, RTOS_BASE, CODE_MAP_END), map((u32)fd, DATA_VMA, DATA_MAP_END), flush };
    struct load_result r;
    u32 status = load(patch, size, &m, &r);
    sys(SYS_CLOSE, (u32)fd, 0, 0, 0, 0, 0);
    if (status) fail(status, r.detail);

    used = 0;
    put("RAMLOAD 1\n");
    line("runs", r.runs);
    line("bytes", r.bytes);
    line("base_crc32", r.base_crc);
    line("patched_crc32", r.patched_crc);
    line("already", r.already);
    save(DIR "LOAD.OK");
    sys(SYS_EXIT, 0, 0, 0, 0, 0, 0);
    for (;;) {}
}
__attribute__((naked)) void _start(void) { __asm__ volatile("b start"); }
