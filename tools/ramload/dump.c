/* RAM dump for the GR III (phase 0 of docs/plan-ram-loader.md): copies the
   running RTOS image and its .data from memory to the SD card. It maps
   /dev/mem read-only and never writes camera memory.

   Runs in the camera's Linux, started by SCRIPT/STARTUP.TTL through `lcmd`.
   No libc: raw EABI syscalls. Modelled on the dump tool of grkit
   (github.com/ahonn/grkit, runtime/dump/dump.c, MIT, Copyright (c) ahonn).

   Output, in the folder of this program:
     RTOS.BIN   0x53000000 up to the end of the image (.data copy source plus
                its size, from the startup literals), rounded up to 4 KiB
     DATA.BIN   .data and BSS as they are at run time (0x55000000 up to the
                BSS end of the literals, rounded up to 4 KiB)
     INFO.TXT   the startup literals, the ranges and a CRC-32 of each file
     DUMP.OK    written last, after a complete dump
     DUMP.ERR   an error code instead of DUMP.OK
   DUMP.LCK admits one run at a time; STARTUP.TTL deletes it first. */
typedef unsigned char u8;
typedef unsigned int u32;

#define RTOS_BASE 0x53000000U
#define DATA_VMA 0x55000000U
/* startup literals: .data source, .data start, .data size, BSS start, BSS end */
#define STARTUP 0x53000450U
#define IDENTITY 0x53002038U
/* never dump more than this from DATA_VMA, whatever the literals say */
#define DATA_LIMIT 0x01000000U
#define DIR "/media/storage1/SCRIPT/RAMLOAD/"

static long sys(u32 n, u32 a, u32 b, u32 c, u32 d, u32 e, u32 f)
{
    register u32 r0 __asm__("r0") = a;
    register u32 r1 __asm__("r1") = b;
    register u32 r2 __asm__("r2") = c;
    register u32 r3 __asm__("r3") = d;
    register u32 r4 __asm__("r4") = e;
    register u32 r5 __asm__("r5") = f;
    register u32 r7 __asm__("r7") = n;
    __asm__ volatile("svc 0" : "+r"(r0) : "r"(r1), "r"(r2), "r"(r3), "r"(r4), "r"(r5), "r"(r7)
                     : "memory", "cc");
    return (long)r0;
}
#define SYS_EXIT 1
#define SYS_WRITE 4
#define SYS_OPEN 5
#define SYS_CLOSE 6
#define SYS_MUNMAP 91
#define SYS_FSYNC 118
#define SYS_MMAP2 192
#define O_RDONLY 0
#define O_WRONLY 1
#define O_CREAT 0100
#define O_EXCL 0200
#define O_TRUNC 01000
#define O_SYNC 04010000
#define PROT_READ 1
#define MAP_SHARED 1

static long mem_fd;

static u32 crc_table[256];
static void crc_init(void)
{
    for (u32 i = 0; i < 256; ++i) {
        u32 c = i;
        for (int k = 0; k < 8; ++k) c = c & 1 ? 0xedb88320U ^ (c >> 1) : c >> 1;
        crc_table[i] = c;
    }
}

static long create(const char *path, u32 flags)
{
    return sys(SYS_OPEN, (u32)path, O_WRONLY | O_CREAT | flags, 0644, 0, 0, 0);
}
static int write_all(long fd, const u8 *p, u32 n)
{
    while (n) {
        long w = sys(SYS_WRITE, (u32)fd, (u32)p, n, 0, 0, 0);
        if (w <= 0) return 0;
        p += w;
        n -= (u32)w;
    }
    return 1;
}

/* a small text buffer for INFO.TXT and DUMP.ERR */
static char text[1024];
static u32 used;
static void put(const char *s) { while (*s && used < sizeof text) text[used++] = *s++; }
static void hex(u32 v)
{
    put("0x");
    for (int i = 7; i >= 0; --i) {
        char c = "0123456789abcdef"[(v >> (i * 4)) & 15];
        if (used < sizeof text) text[used++] = c;
    }
}
static void line(const char *name, u32 v) { put(name); put(" "); hex(v); put("\n"); }
static void save(const char *path)
{
    long fd = create(path, O_TRUNC);
    if (fd >= 0) {
        write_all(fd, (const u8 *)text, used);
        sys(SYS_FSYNC, (u32)fd, 0, 0, 0, 0, 0);
        sys(SYS_CLOSE, (u32)fd, 0, 0, 0, 0, 0);
    }
}
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
        for (u32 i = 0; i < n; ++i) crc = crc_table[(crc ^ bytes[i]) & 0xff] ^ (crc >> 8);
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
