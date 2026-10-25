/* Shared by dump.c and load.c: raw EABI syscalls (no libc) and a small
   text buffer for the result files. The programs run in the camera's
   Linux, started by SCRIPT/STARTUP.TTL through `lcmd`. */
#include "core.h"
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
#define SYS_READ 3
#define SYS_WRITE 4
#define SYS_OPEN 5
#define SYS_CLOSE 6
#define SYS_MUNMAP 91
#define SYS_FSYNC 118
#define SYS_MMAP2 192
/* ARM private: cacheflush(start, end, 0) */
#define SYS_CACHEFLUSH 0xf0002
#define O_RDONLY 0
#define O_WRONLY 1
#define O_RDWR 2
#define O_CREAT 0100
#define O_EXCL 0200
#define O_TRUNC 01000
#define O_SYNC 04010000
#define PROT_READ 1
#define PROT_WRITE 2
#define MAP_SHARED 1


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

/* a small text buffer for the result files */
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
