/* The checks and writes of the RAM loader, without Linux: load.c passes its
   /dev/mem mappings, test-load.c passes buffers on the Mac
   (docs/firmware/ram-loader.md). The format of LOOK.PAT is in
   core/src/rampatch.rs.

   load() checks, before it writes anything:
     - LOOK.PAT: magic, CRC-32, sizes, each run inside code or .data
     - the CRC-32 of the code in RAM (0x53000000..0x5407b540) against the
       base CRC of the patch: the camera runs the firmware the patch was made
       against (T1: this range does not change at run time)
     - the bytes before of every run
   If the RAM already holds the patch (a second run after the same power-on),
   it changes nothing and reports that. It writes the runs in file order;
   if a flush fails, a read-back differs or the code CRC is wrong afterwards,
   it writes every run back to its bytes before.

   Status: 0 loaded, 4 LOOK.PAT damaged (detail: offset), 5 RAM code CRC is
   neither base nor patched (detail: RAM CRC), 6 run outside code/.data
   (detail: address), 7 bytes before do not match (detail: address), 8 some
   runs written, some not (detail: runs found written), 9 flush failed
   (detail: address), 10 read-back differs (detail: address), 11 code CRC
   after writing is wrong (detail: CRC); 9..11 are rolled back. */
#include "core.h"

/* end of code and read-only data: the range of the code CRC */
#define CODE_END 0x5407b540U
#define DATA_SIZE 0x3a080U
#define KIND_FREE 0
#define KIND_DATA 1
#define KIND_CODE 2
#define MAX_RUNS 4096

/* the RAM: code from RTOS_BASE up to CODE_END, .data from DATA_VMA for
   DATA_SIZE bytes; flush makes written bytes visible to the RTOS, 1 = ok */
struct ram {
    volatile u8 *code, *data;
    int (*flush)(volatile u8 *start, u32 n);
};

struct load_result {
    u32 detail, runs, bytes, base_crc, patched_crc, already;
};

/* one run of the patch: its RAM and its bytes before and after */
struct run { u32 address, n; volatile u8 *ram; const u8 *before, *after; };
static struct run runs[MAX_RUNS];

static u32 get32(const u8 *p)
{
    return p[0] | p[1] << 8 | p[2] << 16 | (u32)p[3] << 24;
}

/* RAM for [address, address + n), or 0 outside the range of `kind` */
static volatile u8 *ram_at(const struct ram *m, u32 address, u32 n, u32 kind)
{
    if (kind == KIND_DATA)
        return address >= DATA_VMA && n <= DATA_SIZE && address - DATA_VMA <= DATA_SIZE - n
            ? m->data + (address - DATA_VMA) : 0;
    if (kind == KIND_FREE || kind == KIND_CODE)
        return address >= RTOS_BASE && n <= CODE_END - RTOS_BASE && address - RTOS_BASE <= CODE_END - RTOS_BASE - n
            ? m->code + (address - RTOS_BASE) : 0;
    return 0;
}

static u32 parse(const u8 *patch, u32 size, const struct ram *m, struct load_result *r)
{
    static const char magic[] = "GRLP0001";
    if (size < 0x24 || (size & 3)) return r->detail = size, 4;
    for (u32 i = 0; i < 8; ++i) if (patch[i] != (u8)magic[i]) return r->detail = i, 4;
    u32 end = size - 4;
    if ((crc_update(0xffffffffU, patch, end) ^ 0xffffffffU) != get32(patch + end)) return r->detail = end, 4;
    r->runs = get32(patch + 8);
    u32 total = get32(patch + 12);
    r->base_crc = get32(patch + 16);
    r->patched_crc = get32(patch + 20);
    if (r->runs > MAX_RUNS) return r->detail = 8, 4;
    u32 o = 0x20, sum = 0;
    for (u32 i = 0; i < r->runs; ++i) {
        if (o > end || end - o < 12) return r->detail = o, 4;
        u32 address = get32(patch + o), n = get32(patch + o + 4), kind = get32(patch + o + 8);
        o += 12;
        u32 padded = (n + 3) & ~3U;
        if (!n || n > size || (end - o) / 2 < padded) return r->detail = o, 4;
        volatile u8 *ram = ram_at(m, address, n, kind);
        if (!ram) return r->detail = address, 6;
        runs[i] = (struct run){ address, n, ram, patch + o, patch + o + padded };
        o += 2 * padded;
        sum += n;
    }
    if (o != end || sum != total) return r->detail = o, 4;
    r->bytes = sum;
    return 0;
}

static int equal(volatile const u8 *ram, const u8 *bytes, u32 n)
{
    for (u32 i = 0; i < n; ++i) if (ram[i] != bytes[i]) return 0;
    return 1;
}

/* CRC-32 of the code in RAM, read word by word (device memory: no memcpy) */
static u32 code_crc(const struct ram *m)
{
    static u32 buffer[16384];
    u32 crc = 0xffffffffU;
    for (u32 address = RTOS_BASE; address < CODE_END;) {
        u32 n = CODE_END - address > sizeof buffer ? sizeof buffer : CODE_END - address;
        volatile const u32 *src = (volatile const u32 *)(m->code + (address - RTOS_BASE));
        for (u32 i = 0; i < n / 4; ++i) buffer[i] = src[i];
        crc = crc_update(crc, (const u8 *)buffer, n);
        address += n;
    }
    return crc ^ 0xffffffffU;
}

static int publish(const struct ram *m, const struct run *r, const u8 *bytes)
{
    for (u32 i = 0; i < r->n; ++i) r->ram[i] = bytes[i];
    return m->flush(r->ram, r->n);
}

/* runs [0, n) back to their bytes before, last first */
static void roll_back(const struct ram *m, u32 n)
{
    while (n--) publish(m, &runs[n], runs[n].before);
}

static u32 load(const u8 *patch, u32 size, const struct ram *m, struct load_result *r)
{
    *r = (struct load_result){ 0 };
    crc_init();
    u32 status = parse(patch, size, m, r);
    if (status) return status;

    u32 crc = code_crc(m);
    if (crc != r->base_crc && crc != r->patched_crc) return r->detail = crc, 5;
    u32 as_before = 0, as_after = 0;
    for (u32 i = 0; i < r->runs; ++i) {
        int b = equal(runs[i].ram, runs[i].before, runs[i].n);
        int a = equal(runs[i].ram, runs[i].after, runs[i].n);
        if (!a && !b) return r->detail = runs[i].address, 7;
        as_before += (u32)b;
        as_after += (u32)a;
    }
    r->already = as_after == r->runs && crc == r->patched_crc;
    if (r->already) return 0;
    if (as_before != r->runs || crc != r->base_crc) return r->detail = as_after, 8;

    for (u32 i = 0; i < r->runs; ++i) {
        if (!publish(m, &runs[i], runs[i].after)) {
            roll_back(m, i + 1);
            return r->detail = runs[i].address, 9;
        }
    }
    for (u32 i = 0; i < r->runs; ++i) {
        if (!equal(runs[i].ram, runs[i].after, runs[i].n)) {
            roll_back(m, r->runs);
            return r->detail = runs[i].address, 10;
        }
    }
    crc = code_crc(m);
    if (crc != r->patched_crc) {
        roll_back(m, r->runs);
        return r->detail = crc, 11;
    }
    return 0;
}
