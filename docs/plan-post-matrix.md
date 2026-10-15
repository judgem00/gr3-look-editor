# Plan: post-gamma matrix P per look (data patch)

Goal: a 3×3 colour matrix after the tone curve for one image control, written
as data only. It is needed for spektrafilm film looks
([spektrafilm-fit.md](spektrafilm-fit.md): mean ΔE2000 2.20 → 1.68 with P).
Status 2026-10-15: analysis done (static, on fw 2.10); phases 0–2 done
(emulation, core, check of an exported image). Test firmware built, not yet
flashed. Preview still to do.

## Findings

### The hue tables are shared

`0x55018e4c[look]` (the tone table) points to a set. Its first word points to
the hue table: 9 pointers, one per hue step −4…+4. Each pointer leads to a 3×3
int32 matrix in Q13 (8192 = 1.0).

| Set | Tables | Hue table | Matrices |
|---|---|---|---|
| `0x53dc555c` | 0–5, 7, 9, 11, 12, 17–23, 25 (all colour looks) | `0x5501988c` (.data) | 9 different, ROM `0x53dc5560…0x53dc56a8`, each referenced once |
| `0x53dc5584` | 6, 8, 10, 13–16, 24 (mono looks, HDR variant) | `0x55019868` | one identity matrix (`0x53dc5684`) for all 9 steps |

Every colour look uses the same 9 matrices (a rotation in Cb/Cr, about
2.5° per step). A look with P therefore needs its own set, hue table and
matrices. The two sets are not real structs: set `0x53dc555c` + 4 is
already the matrix for hue +3. **Only the first word is read.**

### Who reads them

- Code reads the tone table once: the setting filler (`0x536dc2a4`) copies
  `0x55018e4c[look]` (look 22 → 25) to `setting+0x18`.
- YCbCrMatrix calc `0x536e4720`: `H = (*setting+0x18)[setting+0x5c]` (hue
  step), `reg = (H × base + 0x1000) >> 13` with base = `0x53dc1444` (int32,
  BT.601, 256 = 1.0: `77 150 29 / −43 −85 128 / 128 −107 −21`).
- Writer `0x536e4760`: 9 registers at info+0x1180…0x1190, `bfi #0, #9`.
  Each coefficient is **9-bit two's complement, 256 = 1.0** (−1.0 … 0.996).
- No other reference to the tone table, the set or the hue table was found
  (literal pools, movw/movt). Phase 2 checks this through the full register
  image in emulation.

### P fits the registers

JPEG decoding inverts BT.601, so storing H(k)·B·P·B⁻¹ (B = base) gives the
camera H(k)·B·P. That is P applied to the gamma-encoded RGB, with the hue
setting still working. For the 7 fitted films, over all 9 hue steps: largest
coefficient 149/255 (stock: 150), rounding at most 0.5/256 (as stock).

## Data patch

One item per target look, 364 bytes, from the space pool
(`space::plan_with`, like the private colour sets):

```
X + 0x00   word   X + 4                     the "set": only word 0 is read
X + 0x04   9 ×    X + 0x28 + 36 k           hue table, step k = 0..8 (−4..+4)
X + 0x28   9 × 9  int32 Q13                 H(k) · B · P · B⁻¹
```

and `0x55018e4c[table] = X` for each of the target's tables (Bleach Bypass:
18, 22, 23). Matrices are rounded so that the register rows come out exact:
Y row sums to 256, Cb and Cr rows to 0, so grey stays grey. The Q13 values
have 32× finer steps than the registers, so this costs no accuracy.

## Phases

0. **Emulation check, before any core code** (`tools/verify-tuning.py`, new
   check): hook the setting filler so that one look's `setting+0x18` points to
   a test set in RAM. Then, for hue −4…+4: CSP registers = round(H·B·P), every
   other register and table of the info block identical to stock, other looks
   untouched, also right after that look. This proves the model and that
   nothing else reads `+0x18`.

   **Done 2026-10-15:** `tools/verify-tuning.py --only q5`, all passed. The
   test writes the data patch into the emulator's RAM (set, hue table, 9
   matrices in the heap; tone table entries in .data); no hook is needed. In
   the emulated info block the registers sit at +0x12A0…+0x12B1 (R2Y image
   +0x120, register +0x1180). Checked, still and live view:
   - stock Standard at hue −4…+4: registers = (H × base + 0x1000) >> 13, so
     the model of the calc holds;
   - Positive Film (R/B swap), Negative Film (Portra 400 fit; look 22 →
     table 25) and Bleach Bypass v1–v3 (luma mix; tables 18, 22, 23): at
     every hue step the registers are exactly H(k)·B·P, with rows 256 / 0 / 0;
   - nothing else in the info block and no gamma table changes for the target;
   - every other colour look equals stock, also right after the target (hue 0
     and +4).
1. **Core** (`core/src/looks.rs` or a new `postmatrix.rs`, `targets.rs`): `post`
   (9 values, rows summing to 1) on a target. Item size 364, writer as above,
   with exact register rows. Tests: round trip, register values via the calc
   formula, grey preserved, Bleach Bypass writes 3 entries, space report.

   **Done 2026-10-15:** `core/src/postmatrix.rs`; `Target::post` (export v2:
   `"post": [9]` per target, kept in the preset; identity or null = none);
   item `Post` in `targets.rs`, written after `inherit_from`. Bleach Bypass
   always gets P on all three tables (`post_tables`), also with its own source.
   Refused: rows that do not sum to 1 (±0.001), and coefficients beyond the
   9-bit registers (error "post matrix too strong"). Tests: `postmatrix`
   unit tests, `core/tests/postmatrix.rs` (the export is exactly stock + item
   + entry + checksum; registers within 1 of H·B·P with rows 256/0/0; other
   looks unchanged; several targets; refusals; space). Unicorn on an exported
   image (Negative Film Portra P, Positive Film swap, Bleach Bypass Portra P):
   all looks, still and live view, hue −4/0/+4 in one history: target
   registers within 1 of H·B·P, nothing else differs from stock.

   Space: an item is 364 bytes and does not fit into the reserved tail (344
   bytes). With every HSL slot taken there is no room for P. It does not
   change how many complete film looks fit (HSL + RGB curves + own set): 3,
   with or without P.
2. **Verification of an exported image** (`--patched`): all 9 hue steps of
   the target and every other look against stock in Unicorn.

   **Done 2026-10-15:** `tools/verify-tuning.py --post IMAGE PRESET`. Every
   look in one history, still and live view, hue −4/0/+4. A target with P
   must equal its source in stock except for the nine registers, and those
   must be within 1 of H·B·P with rows 256/0/0. Every other look must equal
   stock. On the unpatched image it fails, as it should.
   `tools/verify-targets.py --patched` now leaves the YCbCr registers out
   for targets with P. `GR_IMAGE` points `tools/emu.py` at the stock image
   from elsewhere (a worktree).
3. **Preview** (`app.js`): after the tone curve, before the hue rotation,
   `rgb ← P rgb` for a target with P. Already modelled this way:
   `hueMatrix = YCC⁻¹ H YCC`.
4. **Flash test** (below).
5. **Fitter in the editor**: target → matrix + curves + HSL + P (port of
   `tools/spektrafilm-fit.py`). Its own plan after the camera test.

Also fix `docs/image-pipeline.md`: CCA1 sits before the gamma (camera-tested
in gr3-film-mods §22l), its matrix defaults to identity 0x200, and the real
post-gamma stage is this CSP matrix.

## Flash test (one flash)

Built 2026-10-15: `cargo run --release --example post_matrix_flash` →
`work/post-matrix-test/` (FWDC239B.BIN SHA-256 `677773fb…586cb01`, unpackedFW,
preset, HOW_TO_SHOOT.txt). Checked: `fw-pack.py check`, `verify-tuning.py
--post`, `verify-targets.py --patched`.

Following the test protocol: one variant per image control, all on
Standard's colour data, so Standard is the exact control. This flash tests
only P. The comparison of a complete film with and without P needs matrix,
curves and HSL from the fitter, so it moves to the flash after phase 5.

| Image control | Content | Shows |
|---|---|---|
| Standard | unchanged | control |
| Positive Film | Standard + P = R/B swap `[0 0 1; 0 1 0; 1 0 0]` | path works at all (yellow → cyan, unmistakable) |
| Vivid | Standard + P = halfway to grey, `0.5 I + 0.5 [luma rows]` | position: the model after vs before the curve predicts other colours |
| Negative Film | Standard + P = Portra 400 fit | a real, small P as predicted |

Shots: tripod, same light, Daylight WB, fixed exposure, RAW + JPEG, every
slider at 0 in the four image controls, grain off; a grey ramp and the hue
chart in the picture. Order: Standard, Positive Film, Vivid, Negative Film,
then Positive Film, Negative Film and Standard at hue +4, Standard again.

Evaluation on the flashed image (`unpackedFW`), not the build script:

- each variant: camera JPEG vs the Standard model from the DNG with P
  applied after the curve. Its error should stay at Standard's level
  (≈ 0.7 ΔE).
- Positive Film / Vivid: the predicted pattern, grey unchanged. Vivid also
  against "P before the curve": that model must fit clearly worse.
- hue +4: the same P on top of the rotation.

Risk: one ROM pointer per target in `.data` and 364 bytes in the pool.
Everything else is unchanged. A wrong value shows up as a colour shift, not
as a crash (the writer masks every coefficient to 9 bits).
