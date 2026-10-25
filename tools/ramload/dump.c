/* RAM dump for the GR III (phase 0 of docs/plan-ram-loader.md): copies the
   running RTOS image and its .data from memory to the SD card. It maps
   /dev/mem read-only and never writes camera memory.

   Runs in the camera's Linux, started by SCRIPT/STARTUP.TTL through `lcmd`.
   No libc: raw EABI syscalls. Modelled on the dump tool of grkit
   (github.com/ahonn/grkit, runtime/dump/dump.c; MIT, see LICENSE-grkit.txt).

   Output, in the folder of this program:
     RTOS.BIN   0x53000000 up to the end of the image (.data copy source plus
                its size, from the startup literals), rounded up to 4 KiB
     DATA.BIN   .data and BSS as they are at run time (0x55000000 up to the
                BSS end of the literals, rounded up to 4 KiB)
     INFO.TXT   the startup literals, the ranges and a CRC-32 of each file
     DUMP.OK    written last, after a complete dump
     DUMP.ERR   an error code instead of DUMP.OK
   DUMP.LCK admits one run at a time; STARTUP.TTL deletes it first. */
#include "common.h"

#define IDENTITY 0x53002038U
/* never dump more than this from DATA_VMA, whatever the literals say */
#define DATA_LIMIT 0x01000000U

static long mem_fd;

static void fail(u32 code, u32 detail)
{
    used = 0;
    line("error", code);
    line("detail", detail);
    save(DIR "DUMP.ERR");
    sys(SYS_EXIT, code, 0, 0, 0, 0, 0);
    for (;;) {}
}

/* Maps [start, end) read-only. start and end are multiples of 4 KiB. */
static volatile const u32 *map(u32 start, u32 end, u32 code)
{
    long m = sys(SYS_MMAP2, 0, end - start, PROT_READ, MAP_SHARED, (u32)mem_fd, start >> 12);
    if ((u32)m >= 0xfffff001U) fail(code, (u32)m);
    return (volatile const u32 *)m;
}

static u32 buffer[16384];

/* Copies [start, end) to path word by word (device memory: no memcpy) and
   returns the CRC-32 of the bytes. */
static u32 dump(const char *path, u32 start, u32 end, u32 code)
{
    volatile const u32 *mem = map(start, end, code);
    long out = create(path, O_TRUNC);
    if (out < 0) fail(code + 1, (u32)out);
    u32 crc = 0xffffffffU;
    for (u32 address = start; address < end;) {
        u32 n = end - address > sizeof buffer ? sizeof buffer : end - address;
        for (u32 i = 0; i < n / 4; ++i) buffer[i] = mem[(address - start) / 4 + i];
        const u8 *bytes = (const u8 *)buffer;
        crc = crc_update(crc, bytes, n);
        if (!write_all(out, bytes, n)) fail(code + 2, address);
        address += n;
    }
    if (sys(SYS_FSYNC, (u32)out, 0, 0, 0, 0, 0) < 0) fail(code + 3, 0);
    sys(SYS_CLOSE, (u32)out, 0, 0, 0, 0, 0);
    sys(SYS_MUNMAP, (u32)mem, end - start, 0, 0, 0, 0);
    return crc ^ 0xffffffffU;
}

static u32 round_up(u32 v) { return (v + 0xfffU) & ~0xfffU; }

__attribute__((used)) static void start(void)
{
    /* a second run while the first one copies stops here */
    long lock = create(DIR "DUMP.LCK", O_EXCL);
    if (lock < 0) sys(SYS_EXIT, 0, 0, 0, 0, 0, 0);
    sys(SYS_CLOSE, (u32)lock, 0, 0, 0, 0, 0);

    mem_fd = sys(SYS_OPEN, (u32)"/dev/mem", O_RDONLY | O_SYNC, 0, 0, 0, 0);
    if (mem_fd < 0) fail(1, (u32)mem_fd);

    /* the startup literals, from the first page of the image */
    volatile const u32 *head = map(RTOS_BASE, RTOS_BASE + 0x3000, 2);
    u32 source = head[(STARTUP - RTOS_BASE) / 4], data = head[(STARTUP + 4 - RTOS_BASE) / 4];
    u32 size = head[(STARTUP + 8 - RTOS_BASE) / 4], bss_start = head[(STARTUP + 12 - RTOS_BASE) / 4];
    u32 bss_end = head[(STARTUP + 16 - RTOS_BASE) / 4], identity = head[(IDENTITY - RTOS_BASE) / 4];
    sys(SYS_MUNMAP, (u32)head, 0x3000, 0, 0, 0, 0);
    if (data != DATA_VMA || source <= RTOS_BASE || source >= DATA_VMA || !size ||
        size > DATA_VMA - source)
        fail(3, source);
    if (bss_end < DATA_VMA + size || bss_end - DATA_VMA > DATA_LIMIT) fail(4, bss_end);

    crc_init();
    u32 rtos_end = round_up(source + size), data_end = round_up(bss_end);
    u32 rtos_crc = dump(DIR "RTOS.BIN", RTOS_BASE, rtos_end, 10);
    u32 data_crc = dump(DIR "DATA.BIN", DATA_VMA, data_end, 20);

    used = 0;
    put("RAMDUMP 1\n");
    line("rtos_start", RTOS_BASE);
    line("rtos_end", rtos_end);
    line("rtos_crc32", rtos_crc);
    line("data_start", DATA_VMA);
    line("data_end", data_end);
    line("data_crc32", data_crc);
    line("literal_data_source", source);
    line("literal_data_start", data);
    line("literal_data_size", size);
    line("literal_bss_start", bss_start);
    line("literal_bss_end", bss_end);
    line("identity", identity);
    save(DIR "INFO.TXT");
    sys(SYS_CLOSE, (u32)mem_fd, 0, 0, 0, 0, 0);

    used = 0;
    line("rtos_bytes", rtos_end - RTOS_BASE);
    line("data_bytes", data_end - DATA_VMA);
    save(DIR "DUMP.OK");
    sys(SYS_EXIT, 0, 0, 0, 0, 0, 0);
    for (;;) {}
}
__attribute__((naked)) void _start(void) { __asm__ volatile("b start"); }
