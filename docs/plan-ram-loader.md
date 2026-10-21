# Plan: load looks into RAM from the SD card (test path, no flash)

As of 2026-10-21. Status: phase 0 done, T1 passed on the camera
([firmware/ram-loader.md](firmware/ram-loader.md)): the camera runs the
build flashed on 2026-10-18, the free space and all export sites hold their
file values at run time. Phase 1 (patch format and export) done; next is
phase 2, the loader.

## Goal

Test a look on the GR III without flashing it. The editor writes a patch
file. At power-on, a script on the card writes the patch into the memory
of the running RTOS. After power-off the camera runs its firmware unchanged
again.

This is a test path only. The flashed update file stays the way to ship a
look. The test path replaces most test flashes (so far each test costs a
flash, so a series has few variants). It
should also help the firmware research: reading live RTOS memory
(ND in video, the AE gain write at `0x53433500`).

## The idea, taken from grkit

[grkit](https://github.com/ahonn/grkit) (MIT) loads film looks this way on
GR III 2.10, the firmware of `fwdc239b`, the image we edit. The parts we
take over as ideas:

1. **SD scripts on.** Two files on the card root, then power on with MENU
   held. The camera shows "Mode Set Command → Complete" and turns off. The
   setting stays on until the same procedure with `DIS` turns it off.
   - `00078350.588` (GR III; GR IIIx: `00078490.609`) containing
     `[SCRIPT_EN_MODE EN]\r\n` (off: `[SCRIPT_EN_MODE DIS]\r\n`)
   - `DEVELOP.MOD`, the 10 bytes `07 01 2c 1f 10 03 1e 16 05 2d`
2. **`SCRIPT/STARTUP.TTL`** runs at each power-on in the RTOS (a macro
   language like Tera Term's). `sendln 'lcmd_resp on'` and
   `sendln 'lcmd sh /media/storage1/SCRIPT/…'` run shell commands in the
   camera's Linux. `lcmd` fails while Linux has not mounted the card, so
   the script repeats it until a result file appears.
3. **A static ARM Linux program** (no libc, raw `svc 0` syscalls) opens
   `/dev/mem` with `O_SYNC`, maps the RTOS RAM from `0x53000000` and
   writes the patch. grkit's dump tool shows that the RTOS image lies at
   `0x53000000` and its `.data` at `0x55000000`, as `core/src/addr.rs`
   assumes.
4. **Checks before each write:** each patched range must still hold the
   expected bytes, otherwise nothing is written. Payload first, entry
   branches last, `dsb sy` and the ARM `cacheflush` syscall (`0xf0002`)
   after code writes.

We write our own loader and do not ship grkit code. If a file is copied
in part, it keeps grkit's MIT notice.

## Why our looks fit this well

Our export changes the decoded RTOS image only at known places
(`space.rs`, `looks.rs`, `targets.rs`, `grain.rs`, `curvesel.rs`,
`postmatrix.rs`):

- pointer tables in `.data` (`0x55018d94`, `0x55018cf0`, `0x55018e4c`, …)
- private sets, HSL sets and channel curves in the padding
  `0x53000d50..0x53004000`, which no pointer leads into
- a few code stubs and branches (grain gate, curve selector)

All of these are RTOS addresses. So the patch is simply the difference
between the image the export builds and the stock image, in RTOS
addresses. No look needs its own patch code.

## Phases

### Phase 0: read-only, which firmware does the camera run?

No write to camera memory.

- Turn on SD scripts (files above).
- Our own small dump program, `tools/ramload/dump.c`: maps
  `0x53000000..0x55040000` read-only and writes `RTOS.BIN` and `DATA.BIN`
  to the card. grkit's `runtime/dump/dump.c` is the model.
- `tools/ram-compare.py`: compares the dump with the decoded stock image
  (via `addr.rs`, text 1:1, `.data` at `DATA_LMA`) and with the image of
  our last flashed export.

Questions this answers:

1. Does the camera run stock 2.10 or our last flash? The patch must be
   computed against what the camera runs. Simplest: flash stock back once
   before the test series. Otherwise we diff against the image of our
   flashed build.
2. Is the padding `0x53000d50..0x53004000` really untouched at runtime
   (no BSS, no heap)?
3. Have the pointer tables we patch stayed as they are in the file? If the
   firmware writes them at startup, we need to know that.

Done when: the dump matches one of the two images outside of known
runtime data, and the three questions are answered in
`docs/firmware/ram-loader.md` (German, as firmware docs are).

### Phase 1: patch format and export

Done 2026-10-21. As built:

- `core/src/rampatch.rs` compares two images in their **RAM view**:
  `0x53000000` up to the end of the `.data` copy source (`0x540b55c0`).
  Bytes of the copy source go to `.data` at `0x55000000`. A RAM dump
  (`RTOS.BIN`) is such a view already, so it can serve as the base. Runs
  closer than 16 bytes are joined.
- Three kinds, written in this order: **free** (the editor's free space,
  and code that was all zero, i.e. a code cave like `video.rs`
  `HOLD_CAVE`), **data** (`.data`), **code** (everything else, e.g. the
  grain branches). Nothing leads into a free run before the data and code
  runs are written.
- `LOOK.PAT` (format in the module doc): magic `GRLP0001`, run count,
  byte count, CRC-32 of the base code (`0x53000000..0x5407b540`) and of
  the patched code, then per run address, length, kind, before and after
  bytes, then a CRC-32 over everything. The loader can check the base code
  CRC against the RAM before it writes: T1 showed that this range does not
  change at run time.
- `Session::export_ram(body, base)`: the same request as `export`. `base`
  is what the camera runs: an update file (decoded and checked like
  Ricoh's), `RTOS.BIN` of a dump, or none (stock). Refuses a look that
  changes bytes outside the RAM view. The card files (loader, scripts)
  come with phase 2; the export gives `LOOK.PAT` only.
- Editor: export dialog, section "Test without flashing (SD card,
  experimental)": optional file for what the camera runs, button, download
  of `LOOK.PAT`.
- Tests (`core/tests/rampatch.rs`): applying the patch to the base view
  gives the export's view byte for byte, from stock and from another
  build; a dump and the update file it came from give the same patch;
  the kinds come in write order and the grain sites are code runs; a
  damaged file and another build are refused. Against the camera's build
  of 2026-10-18 the base code CRC is `1f906534`, the same as in the dump of
  T1.

### Phase 2: loader on the camera

- `tools/ramload/load.c`, built with the local clang/lld
  (`--target=armv7a-linux-gnueabi -nostdlib -static -Os`), script
  `tools/ramload/build.sh`. The binary is checked in under
  `editor/static/ramload/`, so the editor ships it without a toolchain.
- Order:
  1. Lock file, as with grkit, so one run at a time.
  2. Read `LOOK.PAT`, check magic and CRC.
  3. Map `/dev/mem` `O_RDWR | O_SYNC`.
  4. Check all `before` bytes. A single mismatch: write `LOAD.ERR` with
     the address and stop. Nothing has changed then.
  5. Write data runs, then code payloads, then code entry branches.
     `dsb sy`, `cacheflush` over each code range.
  6. Read back each run. Write `LOAD.OK` with the number of runs and
     bytes.
- `STARTUP.TTL`: polls `lcmd sh …/LOAD.SH` until `LOAD.OK` or `LOAD.ERR`
  exists (about 30 s at most), then ends. Deletes old result files first.

### Phase 3: when does the camera read the data?

The patch lands a few seconds after power-on. Anything the firmware
reads only at startup or copies into its own buffers misses it.

- Find out statically and in Unicorn (`tools/verify-tuning.py`,
  `tools/verify-targets.py`) which of our patched places are read per
  shot (ImageTuning reads `0x55018d94[look]` per Calc) and which only
  once.
- Rule for the tests: after `LOAD.OK`, switch the image control away and
  back once. If that is not enough for some data, the loader can trigger
  the reload itself later. That is open.
- Risk: the RTOS core may still have old instructions in its
  instruction cache after code writes. grkit writes code the same way
  and notes that coherence between the cores needs a test on the camera.
  That is why the first camera test writes data only.

### Phase 4: camera tests

Few runs, Standard as control, each run checked on the recorded image
(as in the flash test protocol):

| Test | Patch | Expected |
|---|---|---|
| T1 | none, dump only (phase 0) | answers to phase 0 |
| T2 | data only: a very visible matrix on one target (e.g. Positive Film) | target changed, Standard unchanged; after power-off both stock |
| T3 | full look: HSL, channel curves, P | JPEG and preview match the editor as after a flash |
| T4 | grain (code stubs) | grain as after a flash; no crash in 20 shots |
| T5 | video with the T3 look | look acts in video too (as with the flash) |

### Phase 5: documentation

- `docs/firmware/ram-loader.md` (German): mechanism, addresses, results
  of phase 0 and the tests, recovery.
- `docs/editor.md`: the new export, briefly.

## Safety and recovery

- Nothing is written to flash, except the SD script setting itself.
- The loader writes only after all checks pass, and only into the runs of
  the patch.
- If the camera hangs after the patch: remove the battery, delete the
  `SCRIPT` folder (or remove the card), power on. The camera then starts
  its firmware unchanged.
- grkit warns that a failed patch can stop the camera during startup. With
  our loader the patch only comes after startup, and without a card the
  script does not run.
- Not together with grkit on one card: both use `SCRIPT/STARTUP.TTL`.

## Open questions

- Does the camera currently run stock or one of our flashes? (Phase 0)
- Does the camera's Linux allow writes to `/dev/mem` for these ranges?
  grkit writes there, so most likely yes. T2 confirms it.
- Which data does the firmware read only at startup? (Phase 3)
- Instruction cache of the RTOS core after code writes. (T4)
- GR IIIx: the same RTOS, other ModeSet file. Not tested at first.

## Not in this plan

- A menu in the camera (grkit's GRK entry) or switching looks on the
  camera.
- Making the RAM path the normal way to ship a look.
