/* What the loader logic, the dump and the host test share: types, RTOS
   addresses and CRC-32. No syscalls, so it also compiles on the Mac
   (test-load.c). */
#ifndef RAMLOAD_CORE_H
#define RAMLOAD_CORE_H
typedef unsigned char u8;
typedef unsigned int u32;

#define RTOS_BASE 0x53000000U
#define DATA_VMA 0x55000000U
/* startup literals: .data source, .data start, .data size, BSS start, BSS end */
#define STARTUP 0x53000450U

static u32 crc_table[256];
static void crc_init(void)
{
    for (u32 i = 0; i < 256; ++i) {
        u32 c = i;
        for (int k = 0; k < 8; ++k) c = c & 1 ? 0xedb88320U ^ (c >> 1) : c >> 1;
        crc_table[i] = c;
    }
}
/* crc starts at 0xffffffff; the result is crc ^ 0xffffffff */
static u32 crc_update(u32 crc, const u8 *p, u32 n)
{
    for (u32 i = 0; i < n; ++i) crc = crc_table[(crc ^ p[i]) & 0xff] ^ (crc >> 8);
    return crc;
}

#endif
