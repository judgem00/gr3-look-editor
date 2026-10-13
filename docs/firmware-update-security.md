# GR III update security — analysis

Result of phase 1 of the colour profile plan: does the camera verify modified
firmware cryptographically, or only for integrity?

**Short answer: no cryptography. Only a checksum that can be recomputed.**

## Approach

Firmware `fwdc239b`, unpacked to `assets/fwdc239b/unpackedFW`. The load
address of the RTOS/DSP blob is known (bias `0x52FEF790`, see
`tools/find-load-address.py`), so string references in the code can be
resolved.

## What looked like a signature — and what it really is

| Finding in the image | Reality |
|---|---|
| `No valid EB signature found` | **FAT file system** — the `0xAA55` boot sector marker in the SD card formatter |
| `%sPRSA -------,0x%08x` | **hardware register** `ProReadUnit` (PR + source address), image pipeline |
| `RSA}`, `{AeS`, `#MD5` | random bytes with wrong 4-byte alignment, not symbol names |
| `CHECKSUM NG`, `checksum error!!` | an **additive checksum**, not a signature comparison |

The entire image contains **no crypto library** — no RSA, SHA, AES, HMAC, no
mbedTLS/OpenSSL/wolfSSL. A signature-verifying updater would need one.

## The update path

```
UserFwUpdateController / FactoryFwUpdateController
   -> src/FwUpdate.cpp
        -> CpuFwUpdate / DspFwUpdate / BothFwUpdate
             -> SRIC protocol -> target bootloader (SAMBA-Z)
                  BOOT DOWNLOAD -> BOOT VERIFY -> CHECKSUM
```

Error messages of the updater (complete): `Low Battery`, `File Bloken`,
`Update Failure`, `Unknown`. **No** signature/trust error message.

Internally the camera loads the file as `C:\FWDC239B.BIN` and checks the
`RICOH` header (the file starts with `RICOH\0\0\0GR III`).

## The check in machine code

Routine at RAM `0x535796c0` / `0x53579790` (file offset ~5,808,000), which
logs `CHECKSUM NG: Target:%02d`:

```
53579708  ldrb r1,[r0]; add r2,r2,r1,lsl#24    ; byte, index % 4 == 0
53579774  ldrb r1,[r0]; add r2,r2,r1,lsl#16    ; index % 4 == 1
53579730  ...          add r2,r2,r1,lsl#8      ; index % 4 == 2
53579744  ...          add r2,r2,r1            ; index % 4 == 3
...
537597c4  ldrb r3,[r5,#15]   ; stored check byte from the header
537597c8  ldrb r2,[fp,#-36]  ; byte just computed
537597cc  cmp  r2,r3
537597d0  beq  ok            ; otherwise -> CHECKSUM NG
```

The bytes are accumulated into a 32-bit register (equivalently: the sum of
the data as big-endian 32-bit words), folded to one byte and compared against
**a single stored byte**. Nothing a modified file could not recompute. The
algorithm is reproduced in `tools/fw-checksum.py`.

## Implications

The update path offers no technical defence against modified firmware. So
**gate 1 of the plan is cautiously positive**: real custom colour profiles
can be flashed in principle.

## Open caveats

1. **Static analysis, no execution.** The finding rests on code that was read
   and on missing crypto strings — strong evidence, but the final proof would
   be running the updater in an emulator against a modified file.
2. **Recovery path unclear.** `SAMBA-Z`/`SRIC` hints at a bootloader mode
   that might allow recovery — this must be confirmed before anyone flashes.
   Without recovery, bricking remains a real risk.
3. **Packing the `.bin` — solved.** See [firmware-packing.md](firmware-packing.md).
   Correction: the byte sum described above belongs to the SRIC update of the
   sub-CPU; the update file itself is protected by two word sums.

---

# Recovery path — analysis

Second part of phase 1: is there a way back if a flash fails?

**Short answer: yes, several signs of recovery — but none confirmed on a real
device. Must be verified in practice before flashing.**

## Boot architecture

Four segments in the image, mapped to separate eMMC partitions (addresses
from the `PARTPARA` header):

| Segment | File offset | Role |
|---|---:|---|
| RTOS/DSP/ISP + tuning | 11.2–16.3 MB | image pipeline, **this is where the colour tables live** |
| bootloader `stubby-sc2000-rtos` | 16.8 MB | loads kernel + DTB from the romfs |
| romfs (kernel, `sc2000.dtb`) | 16.8 MB | |
| cpio (Linux rootfs) | 19.7 MB | |
| **M0 supervisor** (`KB588CPU project`) | 33.0 MB | separate always-on core |

The **M0 supervisor** is the key: a small, always-running core for loading,
charging, power and DSP reset — in its **own partition**, separate from the
main firmware.

## What the update does (brick risk)

`src/CpuFwUpdate.cpp` / `DspFwUpdate.cpp` write **directly to the eMMC**:

```
make partition table -> write partition table
write boot header
write boot image (M0 Code)
write partition data
format resource drive -> mount resource drive
```

This is an **in-place flash**. An interruption in the middle of writing
(battery empty, power-off) is the brick case. That is why the updater checks
the battery beforehand (`Low Battery`) and the project number (`project
number is different!` — modified firmware must keep the project number).

## Signs of recovery

**1. SD_BOOT mode (close to hardware).** The M0 supervisor knows an SD boot
mode and switches into it when the handshake with the main system fails:

```
Selected SD_BOOT MODE.
HS error, SD BOOT mode.
```

It is triggered by button states at power-on (`SWMAIN`, `SWPLAY_LONG` and
others) — the classic Ricoh/Pentax recovery pattern (hold a button + firmware
on the SD card). Since the M0 lives in its own partition, it survives a brick
of the main firmware.

**2. Service mode with dump/restore.** `src/FlashMemoryFactory.cpp` offers a
flash-level tool set:

```
dump drive     -> C:\dump_%s.bin / C:\fwdump.bin   (back up flash)
restore drive  <- load dump file                   (write flash back)
format drive / copy drive / break drive
```

This allows a complete flash backup before an experiment and restoring it
afterwards — a built-in backup.

**3. Factory full update.** Besides the user path there is
`FactoryFwUpdateController` with `DSP & CPU FULL Update FW` — a complete
reflash that overwrites more than the normal update.

## Honest assessment

A recovery mechanism **demonstrably exists** in the code — SD boot of the M0
supervisor, service dump/restore, factory full update. That lowers the brick
risk considerably: the M0 lives separately and has an SD boot path.

**What cannot be proven statically:**

- Whether SD_BOOT restores a completely bricked main system (loads the whole
  firmware from SD) or only the limited M0 function
- The exact button trigger for recovery mode
- Whether the SoC's mask ROM (the unflashable first stage) has a USB download
  mode (the `SAMBA-Z` strings hint at it but are not conclusive)

**Consequence for the approach:** do not flash blindly. Before the first
flash
- research the exact SD_BOOT trigger in the GR service scene,
- take a complete flash backup with the service dump,
- and verify the recovery path on the hardware **before** anything modified
  is written.

Only once the way back is confirmed in practice is a flash experiment
justifiable.
