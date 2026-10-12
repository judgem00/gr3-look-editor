# Die Bildpipeline der GR III — Gesamtkarte

Firmware `fwdc239b`, statische Analyse (Capstone, eigene Skripte), Stand
2026-10-12. Ergänzt [look-mapping.md](look-mapping.md),
[tone-curves.md](tone-curves.md) und multiaxial.md; dort
stehen die Details zu Matrix, Kurven und MultiAxial.

Ziel war, jede Stufe zwischen RAW und YCbCr zu kennen und zu prüfen, ob es
außer MultiAxial eine **farbtonselektive** Stufe gibt (Voraussetzung für
HSL-Regler).

## Kurzfassung

- `ImageTuning` hat 15 Stufen. Jede füllt einen Teil eines Registerabbilds
  (`out+0x120`, 0x5d24 Byte für den R2Y-Kern, dazu eigene Blöcke für CNR und
  Resize). `0x537cd218` kopiert das Abbild feldweise in die Hardware.
- **Pro Bildstil verschieden** sind nur drei Datensätze: Farbmatrix
  (`setting+0x08`), MultiAxial (`+0x0c`) und YCbCr-Matrix/Farbton (`+0x18`).
  Alles andere hängt an ISO, Modus oder Reglern.
- **Farbtonselektiv ist in der Hardware nur MultiAxial.** Alle anderen Stufen
  sind Matrizen, 1D-Kurven, Offsets oder Filter. Kandidaten, die ich geprüft
  und verworfen habe: YBCRV/`ColorReferenceTuning` (immer aus, Zweck unklar),
  CNR-Farbunterdrückung (kann nur entsättigen), IIP-LUT (sechs 1D-Kurven).
- **`CC0CTL`-Modus 3 schaltet nicht MultiAxial ein.** Bit 1 aktiviert die
  `CC0YB…`-Stufe (Offset und Verstärkung, laut GoPro-Patent der
  Schwarzpunktabzug). Gegenprobe an vorhandenen Aufnahmen: Modus 3 mit
  Grau-Matrizen (MR024532) und mit Einheitsmatrizen (MR024533) ergibt
  dasselbe Bild (Chroma 20,2 / 20,9, gleiche Helligkeit). Die
  MultiAxial-Matrizen wirken also nicht.
- **CCA1 ist eine zweite 3×3-Matrix nach dem Gamma.** In jedem Modus ist sie
  aus (`CC1CTL = 0`, Matrix null). Das ist eine freie Stufe, aber nicht
  farbtonselektiv.

## Reihenfolge in der Hardware (R2Y-Kern)

Die Blockadressen stehen im Treiber `R2yCoreUnit` (Konstruktor `0x537d2960`).
Die Namen kommen aus den Dump-Texten der Firmware. Die Reihenfolge deckt
sich mit der RGB→YUV-Einheit, die GoPro-Patente für ihre Socionext-basierten
Chips beschreiben: WB → CC0 → [MCC] → Schwarzpunkt → Tonkurve → Gamma →
CC1 → [3D-LUT] → [MCC] → YUV.

| HW-Block | Name | Abbild | Gefüllt von | Inhalt |
|---|---|---|---|---|
| `+0x0000` | CNTL | – | Treiber | Start/Stopp (Bit 8–9), `+0x10` Bit 0 Kantenfilter. Keine Enables pro Block |
| `+0x0020` | OFG | – | – | Offset, WB-Gains |
| `+0x0080` | CCA0 | `0x000–0x056` | ColorMatrix | `CC0CTL` (2 bit), Matrix 9×12 bit (`0x20`), `CC0YBOF` 3×11 bit (`0x40`), `CC0YBGA` 3×9 bit signed (`0x48`), `CC0YBBD` 2×12 bit (`0x50`), `CCYC` 3×8 bit (`0x54`) |
| `+0x1000` | (kein Dump-Name) | `0x080–0xa34` | MultiAxial | YCbCr-Matrix, Achsen, 60 Matrizen, 360 Bereichswerte, Steuerfelder |
| `+0x2000` | BTC | `0x1080–0x1086` | ToneControl | Steuerung adaptive Tonkurve |
| `+0x2040` | TC | `0x10c0–0x10da` | ToneControl | Tonkurve |
| `+0x2060` | GAM | `0x10e0` | Gamma | Steuerbits; Kurven im LUT-RAM |
| `+0x2080` | CCA1 | `0x1100–0x114a` | ColorMatrix | `CC1CTL`, Matrix 9×int16, weitere Felder. **Immer aus** |
| `+0x2100` | (CCA1/CSC) | `0x1180–0x11a1` | YCbCrMatrix | RGB→YCbCr mit Farbtondrehung (Hue-Regler) |
| `+0x2140` | YNR | `0x11c0–0x11dc` | LumaNr | Luma-Rauschfilter |
| `+0x2180…0x23c0` | EGHW, EGMW, EGLW, EGDT, EGCMP, EGCRV | `0x1200–0x1412` | EdgeEnhancement, CoordinateScale (`+0x23a0`) | Schärfung |
| `+0x2400–0x24bf` | YBCRV | `0x1420–0x14a1` | ColorReference | 3 Teilblöcke, siehe unten. **Immer aus** |
| `+0x2500` | CLPF | `0x14a8–0x14f6` | ColorLpf | Chroma-Tiefpass |
| `+0x2580` | CSP | `0x1528–0x1556` | YCbCrOperation | Cb/Cr-Offsets (Tonungen), Chroma-Grenzen |

Außerhalb des R2Y-Abbilds: `out+0x0` Interpolation (Demosaik), `out+0x5e44`
SuppreFilter (CNR, u. a. `CSPR` = Farbunterdrückung), `out+0x7e44`
ResizeEdge, `out+0x8244` ReduceFilter. Den RAW-Teil (Bayer → Bayer:
Schwarzwert, Shading, Defektpixel, ELA-Rauschfilter) übernehmen getrennte
Klassen (`RawImageTuning`, `PPUTuning`, `PPUNRTuning`, `AFDFTuning`).

### LUT-RAM

Am Ende von `0x537cd218` landen große Tabellen per `memcpy` im LUT-Speicher
des Kerns (Basis aus dem Treiberobjekt `+0x18`):

| LUT-RAM | Abbild | Größe | Herkunft |
|---|---|---|---|
| `0x0000`, `0x8000` | `0x1568`, `0x1d68` | je 1024 × u16 | vermutlich ToneControl (Gain-Tabelle) |
| `0x8800` | `0x2568` | 1024 × u16 | Gamma (wahrscheinlich) |
| `0x9000`, `0x9800`, `0xa000` | `0x2d68`, `0x3568`, `0x3d68` | je 1024 × u16 | Gamma: Kanalkurven R, G, B |
| `0xa800` | `0x4568` | 256 × u16 | offen |
| `0xaa00…0xb000` | `0x4768…0x4d68` | 4 × 256 × u16 | Gamma |
| `0xc000…0xc600` und drei weitere | `0x4f68…0x5b68` | je 256 × u16 | offen |

## Stufen und ihre Daten

Dispatcher: `0x536dd51c` (Standard), `0x536dd8f4`, `0x536dded0` (umgekehrte
Reihenfolge, Sonderfall), `0x536ddd14` (virtuell, Klasse
`ImageTuningAeSensor`; arbeitet auf dem dauerhaften `setting` bei
`ImageTuning+0x14`). Jede Stufe: `Set(this, setting, abbild)` = Calc +
Schreiber.

| Member | Klasse | Set | Calc | Schreiber | Daten (`setting+`) |
|---|---|---|---|---|---|
| +0x154 | Interpolation | `0x536decb4` | – | `0x536de454` | ISO-Tabelle |
| +0x158 | ColorMatrix | `0x536df3d4` | `0x536deecc` | `0x536df13c` | `+0x08` = `0x55018d94[Look]` |
| +0x15c | MultiAxial | `0x536e4470` | `0x536df484` | `0x536df55c` | `+0x0c` = `0x55018cf0[Look]` |
| +0x160 | LumaNr | `0x536e4634` | `0x536e44d0` | `0x536e4518` | – |
| +0x164 | YCbCrMatrix | `0x536e4760` | `0x536e4720` | inline | `+0x18` = `0x55018e4c[Look]` |
| +0x168 | ColorReference | `0x536e4c1c` | – | `0x536e4898` | `+0x28` = `0x53dc5764` (Index 9: `0x53dc5750`) |
| +0x16c | CoordinateScale | `0x536e4de4` | `0x536e4c80` | `0x536e4cd0` | `+0x24` = `0x53dc59cc` |
| +0x170 | ColorLpf | `0x536e5070` | `0x536e4e44` | `0x536e4e94` | `+0x20` (Modustabelle) |
| +0x174 | EdgeEnhancement | `0x536e7348` | `0x536e5864` | `0x536e72ec` | ISO-Tabellen |
| +0x178 | YCbCrOperation | `0x536e77dc` | `0x536e73d0` | `0x536e7610` | `+0x30` = `0x53dce744` / `0x53dcf5ec` |
| +0x17c | Gamma | `0x536e9000` | `0x536e8fbc` | `0x536e7a24` | Kurven, siehe tone-curves.md |
| +0x180 | ToneControl | `0x536e9d1c` | `0x536e9bd8` | `0x536e91a8` | `+0x34`, `+0x40` |
| +0x184 | ResizeEdge | `0x536eaf80` | – | – | ISO-Tabelle |
| +0x188 | SuppreFilter | `0x536ec3e0` | `0x536eb2b0` | `0x536eb344` | – |
| +0x18c | ReduceFilter | `0x536eb0b0` | `0x536eb000` | `0x536eb074` | – |

Setting-Befüller: `0x536dc0f4`. Im Standard-Dispatcher setzt er den
MultiAxial-Laufzeitpfad (`setting+0x94`) fest auf 0. Den Pfad nutzen auch
YCbCrOperation (`+0x94…+0x9a`) und der virtuelle Dispatcher. Ist er aktiv,
ersetzt MultiAxial die Achsen durch `setting+0x9c` (0x40 Byte) und zwei Bänke
mit je 5 Matrizen durch `setting+0xdc` bzw. `+0x100`. Gedacht ist das wohl
für die Werksabstimmung. Wer den Pfad einschaltet, ist offen.

## Einzelbefunde

### CC0CTL und die CC0YB-Stufe

`CC0CTL` hat 2 Bit. Die Firmware schreibt immer 1 (Matrix an,
`0x536def10`). Bit 1 schaltet `CC0YBOF/GA/BD` zu. Die Werte dafür liefert
ColorMatrix aus `0x550192f4` (`+0x0c`, `+0x10`, `+0x18`). Wirkung (MR024545
ff. gegen MR024532/33): Lichter etwa doppelt so hell, Schwarz von 3 auf 18
angehoben. Das passt zu Offset plus Verstärkung, wie beim
„Blackpoint-RGB"-Block im Patent. Mit MultiAxial hat das nichts zu tun.

### CCA1 (zweite Matrix)

Im Konstruktor (`0x536dece8`) wird `CC1CTL` (+0x30) auf 0 gesetzt und die
Matrix (+0x32…+0x42) auf 0. Calc schreibt dort nie. Die Stufe sitzt hinter
dem Gamma und arbeitet also auf gamma-kodiertem RGB. Ein Test wäre billig:
`CC1CTL = 1` und eine Einheitsmatrix (Skala testen), danach eine
Farbmischung im Gamma-Raum. Für HSL reicht das nicht.

### ColorReference → YBCRV

Drei Datensätze: 8 Byte Steuerung (`+0x2400`), dazu zwei gleich gebaute
Teilblöcke (`+0x2440`, `+0x2480`). Jeder Teilblock besteht aus:

- Enable-Bits (Bit 0 und Bit 4)
- einem 2-bit-Modus
- Punkten (2 × 11 bit)
- einer 7-bit-Breite
- drei Bytepaaren (Unter-/Obergrenze?)
- 9-bit-Werten (Standard 0 und 0x100 = 1,0)

In allen Datensätzen stehen die Enables auf 0. Für Index 9 (anderer Modus)
weicht nur ein 9-bit-Wert ab (0x80 statt 0x100). Nach der Lage im Kern
(zwischen Kantenkurve und Chroma-Tiefpass) dient die Stufe eher der
luminanz-/kantenabhängigen Chroma-Steuerung als einer Farbkorrektur. Ohne
Hardware-Doku bleibt der Zweck unbestätigt.

### YCbCrOperation → CSP

Calc rechnet aus vier 16-bit-Werten (Tabellen über `setting+0x60`) mit den
Faktoren 249/−11 Cb/Cr-Offsets aus. Das sind die **Tonungen** der
Monochrom-Looks (Sepia, Blau …). Kandidat für das offene „+0x797
Toning"-Feld.

### IIP-LUT

Der IIP-Prozessor (Wiedergabe/Retusche, Mehrbild-HDR) hat eine LUT-Einheit
mit `LUTCTL_A…F` und je vier `LUTPRECAL`. Das sind sechs 1D-Kurven, keine
3D-LUT. Für die Aufnahme irrelevant.

## Was das für HSL heißt

1. **MultiAxial bleibt der einzige Hardwareweg.** Die Stufe sitzt (wie im
   Patent) direkt hinter CC0 im linearen RGB. Jede getestete Einstellung
   blieb wirkungslos. Ihr Enable liegt nicht in `CNTL` und nicht in
   `CC0CTL`, also vermutlich im Block selbst (Steuerfelder `0xa00…0xa34`,
   21 Felder, nur eins getestet) oder in einem Taktgating außerhalb des
   R2Y-Treibers. Der nächste statische Schritt wäre der virtuelle
   Dispatcher `0x536ddd14` bzw. der Laufzeitpfad (`setting+0x94`): Wenn
   die Werksabstimmung MultiAxial benutzt, dann über diesen Pfad, und wer
   ihn befüllt, setzt womöglich auch die übrigen Felder.
2. **Annäherung ohne MultiAxial:** Nur getrennt nach Helligkeit möglich
   (Kanalkurven, Farbräder), nicht nach Farbton. Das ist schon im Editor.
3. **Ungenutzte Stufen** (CCA1, YBCRV, CC0YB) erweitern die
   Matrix-/Offset-Werkzeuge, ersetzen aber kein HSL.

## Werkzeuge

Die Analyse-Skripte (Capstone, Branch-Index, symbolische Auswertung von
`0x537cd218`) lagen im Session-Scratchpad und sind nicht im Repo. Die
Methode zum Nachbauen:

- Setter `0x537d2xxx…0x537dcxxx` lesen `ldr r2,[r0]` und dann
  `[r2,#reg]`. Der Handle-Offset im Treiberobjekt ergibt den Block.
- Die Schreiber der Tuning-Klassen bekommen das Abbild in `r1`. Die
  Feldoffsets sind direkte `strb/strh [r1|r1+0x1000, #off]`.
