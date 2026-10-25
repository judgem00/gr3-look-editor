/* ND monitor for the GR III (docs/firmware/nd-monitor.md): watches the
   fields that decide whether the ND filter moves, while the user presses the
   ND button in photo P, movie P and movie M. It maps /dev/mem read-only and
   never writes camera memory, like DUMP.BIN.

   Runs in the camera's Linux, started in the background by NDMON.SH, which
   STARTUP.TTL calls through `lcmd`. No libc: raw EABI syscalls.

   Output, in the folder of this program:
     NDMON.LOG  start line, every change of a watched value with its time in
                ms since the start, and all values every 10 s
     SNAP.BIN   records {"SNAP", ms, address, length} + bytes: the AE
                context's ND area and the LiveViewProcess object at the start,
                every 10 s and after each change (at most one per second);
                .data and BSS at the start and the end
                Lines "d ADDRESS old -> new": word diffs of whole areas, see
                diff_all; "d!" marks the last line of a noisy word
     NDMON.OK   written at the end (RUN_MS), or NDMON.ERR with an error code
   NDMON.LCK admits one run at a time; STARTUP.TTL deletes it first.

   Fields (addresses from the GR III 2.10 image, docs/firmware/video.md):
     ND driver object 0x5536dc58: +0x38 1 = filter in; the driver counts
       each move at [[[obj+0x28]+4]+0x54] (0x536f16c0)
     AE service 0x55085104: AE context = [+0x10]; its ND state machine
       0x5344d090 reads +0x555c..+0x555d, +0x5684, +0x5694 (request bits),
       +0x56a0 (filter in?) and the globals 0x550850c1/c3/fc
     UserData (model 0x55063f30, data [+4]): +0x44 1 = movie, +0x172
       StillNdFilter, +0x530 MovieNdFilter (1 on, 2 off)
     LiveViewProcess (vtable 0x53c1d000, found on the heap): +0x728 flags
       (0x10 drive released), +0x730 ND state, +0x760, +0x9f0c..+0x9f0d
       (ND change pending, 0x534448f8), +0xac4, AE context at +0xaa0 */
#include "common.h"

#define IDENTITY 0x53002038U
#define BUILD_ID 0x1b150a02U
#define BSS_END 0x55804374U
#define MAP_START DATA_VMA
/* the object heap: run 2 found the AE context at 0x597cdcf0 and most
   pointers of .data in 0x59600000..0x59d00000, not in the newlib heap
   0x558043c0..0x57004380 of the startup code */
#define MAP_END 0x5a000000U
#define HEAP_START 0x59000000U
#define HEAP_END 0x5a000000U

#define ND_OBJ 0x5536dc58U
#define ND_PTR 0x5508550cU
#define AE_SERVICE 0x55085104U
#define LVP_VTABLE 0x53c1d000U
#define LVP_SIZE 0xb000U
#define USERDATA 0x55063f30U /* model; its data at [+4] */

#define RUN_MS (3U * 60U * 1000U)
#define POLL_MS 20U
#define SNAP_MAX (400U << 20)

#define SYS_FORK 2
#define SYS_SETSID 66
#define SYS_NANOSLEEP 162
#define SYS_RT_SIGACTION 174
#define SYS_CLOCK_GETTIME 263

static long mem_fd, log_fd = -1, snap_fd = -1;
static volatile const u32 *mem;
/* the UserData data (0x5d520c80 in run 6) lies far above the object heap;
   mapping up to there killed run 7 without a trace, so only its two pages
   are mapped, once, at the address the model holds at the start */
static volatile const u32 *ud_mem;
static u32 ud_base;
static u32 snap_bytes;

static void fail(u32 code, u32 detail)
{
    used = 0;
    line("error", code);
    line("detail", detail);
    save(DIR "NDMON.ERR");
    sys(SYS_EXIT, code, 0, 0, 0, 0, 0);
    for (;;) {}
}

static u32 now_ms(void)
{
    u32 ts[2];
    sys(SYS_CLOCK_GETTIME, 1, (u32)ts, 0, 0, 0, 0); /* CLOCK_MONOTONIC */
    return ts[0] * 1000U + ts[1] / 1000000U;
}

static void sleep_ms(u32 ms)
{
    u32 ts[2] = {ms / 1000U, (ms % 1000U) * 1000000U};
    sys(SYS_NANOSLEEP, (u32)ts, 0, 0, 0, 0, 0);
}

/* a line straight into NDMON.LOG, synced: progress before the first
   sample, and what a signal handler reports. Own buffer, so it does not
   touch a half-written `text`. */
static void note(const char *label, u32 v)
{
    char b[64];
    u32 n = 0;
    while (*label && n < 40) b[n++] = *label++;
    b[n++] = ' ';
    for (int i = 7; i >= 0; --i) b[n++] = "0123456789abcdef"[(v >> (i * 4)) & 15];
    b[n++] = '\n';
    if (log_fd < 0) return;
    write_all(log_fd, (const u8 *)b, n);
    sys(SYS_FSYNC, (u32)log_fd, 0, 0, 0, 0, 0);
}

/* A bad read of /dev/mem ends in SIGBUS or SIGSEGV: report the address
   (siginfo si_addr) instead of dying silently. Signals that would stop
   the run from outside (hangup, terminate when lcmd ends) are logged and
   ignored. */
static void on_fault(int sig, u32 *info, void *context)
{
    (void)context;
    note("fault signal", (u32)sig);
    note("fault address", info[3]);
    fail(0x100U + (u32)sig, info[3]);
}
static void on_signal(int sig) { note("signal ignored", (u32)sig); }
static void handle(u32 sig, void *handler, u32 flags)
{
    u32 act[5] = {(u32)handler, flags, 0, 0, 0};
    sys(SYS_RT_SIGACTION, sig, (u32)act, 0, 8, 0, 0);
}
#define SA_SIGINFO 4U
#define SA_RESTART 0x10000000U

/* words of the mapped RTOS data, heap included; anything else reads as BAD */
#define BAD 0xffffffffU
static int mapped(u32 a, u32 n) { return a >= MAP_START && a <= MAP_END - n; }
static u32 rd32(u32 a) { return mapped(a, 4) && !(a & 3) ? mem[(a - MAP_START) / 4] : BAD; }
static u32 rd8(u32 a) { return mapped(a, 4) ? (rd32(a & ~3U) >> ((a & 3) * 8)) & 0xff : BAD; }
static u32 ud8(u32 a)
{
    if (!ud_mem || a < ud_base || a >= ud_base + 0x2000) return BAD;
    return (ud_mem[(a - ud_base) / 4] >> ((a & 3) * 8)) & 0xff;
}

/* the log: the text buffer of common.h, appended to NDMON.LOG */
static void flush_log(int sync)
{
    if (used && log_fd >= 0) write_all(log_fd, (const u8 *)text, used);
    used = 0;
    if (sync && log_fd >= 0) sys(SYS_FSYNC, (u32)log_fd, 0, 0, 0, 0, 0);
}
static void dec(u32 v, int width)
{
    char d[10];
    int n = 0;
    do d[n++] = (char)('0' + v % 10); while ((v /= 10) && n < 10);
    while (width-- > n) put("0");
    while (n) {
        char s[2] = {d[--n], 0};
        put(s);
    }
}
static void stamp(u32 t)
{
    if (used > sizeof text - 128) flush_log(0);
    dec(t / 1000U, 6);
    put(".");
    dec(t % 1000U, 3);
    put(" ");
}

/* the LiveViewProcess object: a heap word holding its vtable whose +0xaa0
   is the AE context. 0 until found. */
static u32 lvp;

static void find_lvp(u32 t, u32 ae)
{
    if (!mapped(ae, 4)) return;
    for (u32 a = HEAP_START & ~3U; a < HEAP_END - LVP_SIZE; a += 4) {
        if (!(a & 0x3fffffU)) note("scan", a);
        if (mem[(a - MAP_START) / 4] != LVP_VTABLE || rd32(a + 0xaa0) != ae) continue;
        lvp = a;
        stamp(t);
        line("lvp", lvp);
        return;
    }
}

enum {
    V_ND_PTR, V_ND_STATE, V_ND_MOVES, V_G_C0, V_G_FC, V_AE,
    V_AE_5558, V_AE_555C, V_AE_5684, V_AE_5690, V_AE_5694, V_AE_5698, V_AE_569C, V_AE_56A0,
    V_AE_56A4, V_AE_56A8, V_AE_B6E4, V_LVP,
    V_LVP_728, V_LVP_72C, V_LVP_730, V_LVP_750, V_LVP_754, V_LVP_760, V_LVP_764, V_LVP_768,
    V_LVP_9F08, V_LVP_9F0C, V_LVP_AC0, V_LVP_AC4, V_A94_24F8, V_A94_2518,
    V_UD_44, V_UD_172, V_UD_530,
    V_COUNT
};
static const char *const names[V_COUNT] = {
    "nd_ptr", "nd_state", "nd_moves", "g_550850c0", "g_550850fc", "ae",
    "ae+5558", "ae+555c", "ae+5684", "ae+5690", "ae+5694", "ae+5698", "ae+569c", "ae+56a0",
    "ae+56a4", "ae+56a8", "ae+b6e4", "lvp",
    "lvp+728", "lvp+72c", "lvp+730", "lvp+750", "lvp+754", "lvp+760", "lvp+764", "lvp+768",
    "lvp+9f08", "lvp+9f0c", "lvp+ac0", "lvp+ac4", "a94+24f8", "a94+2518",
    "ud+44", "ud+172", "ud+530",
};

static void sample(u32 *v)
{
    v[V_ND_PTR] = rd32(ND_PTR);
    v[V_ND_STATE] = rd8(ND_OBJ + 0x38);
    v[V_ND_MOVES] = rd32(rd32(rd32(ND_OBJ + 0x28) + 4) + 0x54);
    v[V_G_C0] = rd32(0x550850c0);
    v[V_G_FC] = rd32(0x550850fc);
    u32 ae = rd32(AE_SERVICE + 0x10);
    v[V_AE] = ae;
    static const u32 ae_off[] = {0x5558, 0x555c, 0x5684, 0x5690, 0x5694, 0x5698, 0x569c, 0x56a0,
                                 0x56a4, 0x56a8, 0xb6e4};
    for (u32 i = 0; i < sizeof ae_off / 4; ++i) v[V_AE_5558 + i] = rd32(ae + ae_off[i]);
    v[V_LVP] = lvp;
    static const u32 lvp_off[] = {0x728, 0x72c, 0x730, 0x750, 0x754, 0x760, 0x764, 0x768,
                                  0x9f08, 0x9f0c, 0xac0, 0xac4};
    for (u32 i = 0; i < sizeof lvp_off / 4; ++i) v[V_LVP_728 + i] = lvp ? rd32(lvp + lvp_off[i]) : BAD;
    u32 a94 = lvp ? rd32(lvp + 0xa94) : BAD;
    v[V_A94_24F8] = rd32(a94 + 0x24f8);
    v[V_A94_2518] = rd32(a94 + 0x2518);
    /* UserData: +0x44 still/movie, +0x172 StillNdFilter, +0x530 MovieNdFilter */
    u32 ud = rd32(USERDATA + 4);
    v[V_UD_44] = ud8(ud + 0x44);
    v[V_UD_172] = ud8(ud + 0x172);
    v[V_UD_530] = ud8(ud + 0x530);
}

static void snap_range(u32 t, u32 start, u32 n)
{
    static u32 buffer[16384];
    if (snap_fd < 0 || !mapped(start, n) || (start & 3) || (n & 3) || snap_bytes + n + 16 > SNAP_MAX)
        return;
    u32 head[4] = {0x50414e53U /* "SNAP" */, t, start, n};
    write_all(snap_fd, (const u8 *)head, sizeof head);
    for (u32 done = 0; done < n;) {
        u32 k = n - done > sizeof buffer ? sizeof buffer : n - done;
        for (u32 i = 0; i < k / 4; ++i) buffer[i] = mem[(start + done - MAP_START) / 4 + i];
        write_all(snap_fd, (const u8 *)buffer, k);
        done += k;
    }
    snap_bytes += n + 16;
}

static void snap_objects(u32 t, u32 ae)
{
    snap_range(t, ae + 0x5000, 0x800);
    snap_range(t, ae + 0xb600, 0x200);
    if (lvp) {
        snap_range(t, lvp, LVP_SIZE);
        u32 a94 = rd32(lvp + 0xa94);
        if (mapped(a94 + 0x2400, 0x200)) snap_range(t, (a94 + 0x2400) & ~3U, 0x200);
    }
    snap_range(t, ND_OBJ, 0x80);
}

/* Word diffs of whole areas, to find fields that are not in the list:
   the ND area of the AE context, LiveViewProcess and the ND object every
   poll, .data and BSS one 64th per poll (a full pass about every 1.5 s).
   A word that changes more than NOISY times is reported once as noisy
   and then left out. */
#define NOISY 8U
/* a text log ring of the RTOS in BSS (run 4: ASCII entries filling
   0x553cf74c..0x55543474, each word once, no code reference inside):
   left out, it would drown every other line */
#define LOG_RING_START 0x553c0000U
#define LOG_RING_END 0x55560000U
/* at most this many diff lines per poll, the rest is counted */
#define DIFF_CAP 128U
#define DATA_WORDS ((((BSS_END + 0xfffU) & ~0xfffU) - DATA_VMA) / 4)
#define DATA_SLICES 64U
enum { A_AE, A_AE_B6, A_LVP, A_ND, A_DATA, A_COUNT };
static const u32 area_words[A_COUNT] = {0x800 / 4, 0x200 / 4, LVP_SIZE / 4, 0x80 / 4, DATA_WORDS};
static u32 area_base[A_COUNT];
static u32 shadow[0x800 / 4 + 0x200 / 4 + LVP_SIZE / 4 + 0x80 / 4 + DATA_WORDS];
static u8 changes[sizeof shadow / 4];
static u32 diff_lines, poll_lines, dropped;

static u32 *area_shadow(u32 a, u8 **count)
{
    u32 o = 0;
    for (u32 i = 0; i < a; ++i) o += area_words[i];
    *count = changes + o;
    return shadow + o;
}

/* base 0 (object not found) stops the area; a new base starts it fresh */
static void diff(u32 t, u32 a, u32 base, u32 from, u32 to)
{
    u8 *count;
    u32 *old = area_shadow(a, &count);
    if (!mapped(base, area_words[a] * 4)) base = 0;
    if (base != area_base[a]) {
        area_base[a] = base;
        for (u32 i = 0; i < area_words[a]; ++i) {
            old[i] = base ? mem[(base - MAP_START) / 4 + i] : 0;
            count[i] = 0;
        }
        return;
    }
    if (!base) return;
    for (u32 i = from; i < to; ++i) {
        u32 w = mem[(base - MAP_START) / 4 + i];
        if (w == old[i]) continue;
        u32 address = base + i * 4;
        if (address >= LOG_RING_START && address < LOG_RING_END) continue;
        if (count[i] < 255) ++count[i];
        if (count[i] <= NOISY && poll_lines >= DIFF_CAP) {
            ++dropped;
        } else if (count[i] <= NOISY) {
            ++poll_lines;
            ++diff_lines;
            stamp(t);
            put(count[i] == NOISY ? "d! " : "d ");
            hex(address);
            put(" ");
            hex(old[i]);
            put(" -> ");
            hex(w);
            put("\n");
        }
        old[i] = w;
    }
}

static void diff_all(u32 t, u32 ae, u32 slice)
{
    poll_lines = 0;
    diff(t, A_AE, ae ? ae + 0x5000 : 0, 0, area_words[A_AE]);
    diff(t, A_AE_B6, ae ? ae + 0xb600 : 0, 0, area_words[A_AE_B6]);
    diff(t, A_LVP, lvp, 0, area_words[A_LVP]);
    diff(t, A_ND, ND_OBJ, 0, area_words[A_ND]);
    u32 n = (DATA_WORDS + DATA_SLICES - 1) / DATA_SLICES, from = slice * n;
    diff(t, A_DATA, DATA_VMA, from, from + n > DATA_WORDS ? DATA_WORDS : from + n);
    if (dropped && poll_lines < DIFF_CAP) {
        stamp(t);
        put("d+ ");
        hex(dropped);
        put(" more\n");
        dropped = 0;
    }
}

static void all(u32 t, const u32 *v)
{
    stamp(t);
    put("all");
    for (u32 i = 0; i < V_COUNT; ++i) {
        if (used > sizeof text - 64) flush_log(0);
        put(" ");
        put(names[i]);
        put("=");
        hex(v[i]);
    }
    put("\n");
}

__attribute__((used)) static void start(void)
{
    long lock = create(DIR "NDMON.LCK", O_EXCL);
    if (lock < 0) sys(SYS_EXIT, 0, 0, 0, 0, 0, 0);
    sys(SYS_CLOSE, (u32)lock, 0, 0, 0, 0, 0);

    /* leave lcmd's shell: own session, the parent returns at once */
    long pid = sys(SYS_FORK, 0, 0, 0, 0, 0, 0);
    if (pid < 0) fail(7, (u32)pid);
    if (pid > 0) sys(SYS_EXIT, 0, 0, 0, 0, 0, 0);
    sys(SYS_SETSID, 0, 0, 0, 0, 0, 0);
    handle(7, (void *)on_fault, SA_SIGINFO);  /* SIGBUS */
    handle(11, (void *)on_fault, SA_SIGINFO); /* SIGSEGV */
    handle(4, (void *)on_fault, SA_SIGINFO);  /* SIGILL */
    handle(1, (void *)on_signal, SA_RESTART); /* SIGHUP */
    handle(2, (void *)on_signal, SA_RESTART); /* SIGINT */
    handle(13, (void *)on_signal, SA_RESTART); /* SIGPIPE */
    handle(15, (void *)on_signal, SA_RESTART); /* SIGTERM */

    mem_fd = sys(SYS_OPEN, (u32)"/dev/mem", O_RDONLY | O_SYNC, 0, 0, 0, 0);
    if (mem_fd < 0) fail(1, (u32)mem_fd);

    /* the build: identity word and BSS end must be those of 2.10 */
    long head = sys(SYS_MMAP2, 0, 0x3000, PROT_READ, MAP_SHARED, (u32)mem_fd, RTOS_BASE >> 12);
    if ((u32)head >= 0xfffff001U) fail(2, (u32)head);
    volatile const u32 *h = (volatile const u32 *)head;
    u32 identity = h[(IDENTITY - RTOS_BASE) / 4], bss_end = h[(STARTUP + 16 - RTOS_BASE) / 4];
    sys(SYS_MUNMAP, (u32)head, 0x3000, 0, 0, 0, 0);
    if (identity != BUILD_ID) fail(3, identity);
    if (bss_end != BSS_END) fail(4, bss_end);

    long m = sys(SYS_MMAP2, 0, MAP_END - MAP_START, PROT_READ, MAP_SHARED, (u32)mem_fd, MAP_START >> 12);
    if ((u32)m >= 0xfffff001U) fail(5, (u32)m);
    mem = (volatile const u32 *)m;

    u32 ud = rd32(USERDATA + 4);
    if (ud != BAD && ud >= 0x50000000U && ud < 0x60000000U) {
        long um = sys(SYS_MMAP2, 0, 0x2000, PROT_READ, MAP_SHARED, (u32)mem_fd, ud >> 12);
        if ((u32)um < 0xfffff001U) {
            ud_mem = (volatile const u32 *)um;
            ud_base = ud & ~0xfffU;
        }
    }

    log_fd = create(DIR "NDMON.LOG", O_TRUNC);
    if (log_fd < 0) fail(6, (u32)log_fd);
    snap_fd = create(DIR "SNAP.BIN", O_TRUNC);

    u32 real[2];
    sys(SYS_CLOCK_GETTIME, 0, (u32)real, 0, 0, 0, 0); /* CLOCK_REALTIME */
    u32 t0 = now_ms();
    put("NDMON 1 realtime ");
    dec(real[0], 1);
    put(" poll_ms ");
    dec(POLL_MS, 1);
    put("\n");
    flush_log(1);

    u32 v[V_COUNT], old[V_COUNT];
    u32 ae = rd32(AE_SERVICE + 0x10);
    note("ae", ae);
    note("userdata", ud_base ? rd32(USERDATA + 4) : BAD);
    note("nd_state", rd8(ND_OBJ + 0x38));
    find_lvp(0, ae);
    flush_log(1);
    note("scan done", now_ms() - t0);
    sample(v);
    all(0, v);
    flush_log(1);
    snap_objects(0, ae);
    snap_range(0, DATA_VMA, ((BSS_END + 0xfffU) & ~0xfffU) - DATA_VMA);
    note("first snapshot done", now_ms() - t0);
    diff_all(0, ae, 0);
    note("diff ready", now_ms() - t0);

    u32 last_all = 0, last_snap = 0, last_sync = 0, last_find = 0, slice = 0;
    for (;;) {
        sleep_ms(POLL_MS);
        u32 t = now_ms() - t0;
        if (t >= RUN_MS) break;
        for (u32 i = 0; i < V_COUNT; ++i) old[i] = v[i];
        ae = rd32(AE_SERVICE + 0x10);
        if ((!lvp || rd32(lvp) != LVP_VTABLE || rd32(lvp + 0xaa0) != ae) && t - last_find >= 10000) {
            lvp = 0;
            find_lvp(t, ae);
            last_find = t;
        }
        sample(v);
        u32 before = diff_lines;
        slice = (slice + 1) % DATA_SLICES;
        diff_all(t, ae, slice);
        int changed = diff_lines != before;
        for (u32 i = 0; i < V_COUNT; ++i) {
            if (v[i] == old[i]) continue;
            stamp(t);
            put(names[i]);
            put(" ");
            hex(old[i]);
            put(" -> ");
            hex(v[i]);
            put("\n");
            changed = 1;
        }
        if (t - last_all >= 10000) {
            all(t, v);
            last_all = t;
        }
        if ((changed && t - last_snap >= 1000) || t - last_snap >= 10000) {
            snap_objects(t, ae);
            last_snap = t;
        }
        if (changed || t - last_sync >= 2000) {
            flush_log(t - last_sync >= 2000);
            if (t - last_sync >= 2000) {
                if (snap_fd >= 0) sys(SYS_FSYNC, (u32)snap_fd, 0, 0, 0, 0, 0);
                last_sync = t;
            }
        }
    }
    all(now_ms() - t0, v);
    flush_log(1);
    snap_objects(now_ms() - t0, ae);
    snap_range(now_ms() - t0, DATA_VMA, ((BSS_END + 0xfffU) & ~0xfffU) - DATA_VMA);
    if (snap_fd >= 0) {
        sys(SYS_FSYNC, (u32)snap_fd, 0, 0, 0, 0, 0);
        sys(SYS_CLOSE, (u32)snap_fd, 0, 0, 0, 0, 0);
    }
    sys(SYS_CLOSE, (u32)log_fd, 0, 0, 0, 0, 0);
    used = 0;
    line("lvp", lvp);
    line("snap_bytes", snap_bytes);
    save(DIR "NDMON.OK");
    sys(SYS_EXIT, 0, 0, 0, 0, 0, 0);
    for (;;) {}
}
__attribute__((naked)) void _start(void) { __asm__ volatile("b start"); }
