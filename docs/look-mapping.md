# Bildstil → Daten: die Zuordnung

Phase 3 des Farbprofil-Plans: Welche Daten gehören zu
welchem Bildstil? Für die **Farbmatrix** ist die Kette jetzt vollständig im
Code belegt.

Firmware `fwdc239b` (GR III), Datei `assets/fwdc239b/unpackedFW`.

## Kurzfassung

```
Bildstil (UI)            ConvertToCommonPropertyImageControl  0x53289058
  -> interner Index      Byte setting+0x77c (11 … 22)
  -> 0x55018d94[Index]   26 Zeiger, einer pro Index (.data)
       -> ColorMatrix-Satz   +0x00 Sensor-CCM (für alle Looks gleich)
                             +0x04 Look-Matrix   <- der Charakter des Looks
  -> ColorMatrixTuning::Calc 0x536deecc -> ISP-Register
```

Werkzeug: `tools/look-matrices.py` zeigt die
Matrix jedes Looks und schreibt eine geänderte in eine Kopie des Images.

## 1. Zweite Adressabbildung: `.data` liegt bei `0x55000000`

Der Startup-Code (`0x5300038c`) kopiert vor allem anderen `0x3a080` Bytes von
`0x5407b540` nach **`0x55000000`** und nullt danach BSS
(`0x5503a100 … 0x55804374`).

```
RTOS-Code/rodata:  RAM = Datei-Offset + 0x52FEF790
.data:             RAM 0x55000000 + x  =  Datei-Offset (0x5407b540 - 0x52FEF790) + x
```

Folgen für die bisherigen Funde:

- Die „Tuning-Registry" bei Datei-Offset 17.454.552 **ist** diese
  `.data`-Sektion (RAM `0x55019828`). Deshalb hatte sie keine statischen
  Referenzen — der Code spricht sie unter `0x5501…` an.
- Zeiger nach `0x5501…`/`0x5502…` sind keine Laufzeitpuffer, sondern
  initialisierte Daten aus dem Image — und damit patchbar.

## 2. Die Klasse `ImageTuning`

Konstruktor bei `0x536da2xx` (`src/ImageTuning.cpp`), Member laut Asserts:

| Offset | Member | Init |
|---|---|---|
| +0x154 | InterpolationTuning | `0x536de438` |
| +0x158 | **ColorMatrixTuning** | `0x536ded68` |
| +0x15c | **MultiAxialTuning** | `0x536df468` |
| +0x164 | YCbCrMatrixTuning | `0x536e4694` |
| +0x168 | ColorReferenceTuning | `0x536e487c` |
| +0x17c | **GammaTuning** (0x3240 B) | `0x536e7a10` |
| +0x180 | **ToneControlTuning** | `0x536e90ac` |
| +0x204 / +0x208 | BasicGammaTable Natural / Reversal | – |

Jede Tuning-Klasse bekommt ein `setting`-Struct; dessen Zeigerfelder füllt die
Funktion ab `0x536dc0f4` — **indiziert mit dem Bildstil**.

## 3. ColorMatrixTuning::Calc (`0x536deecc`)

1. WB-Gains (`setting+0x7c/7e/80`) × **Look-Matrix** (`set+0x04`)
2. optional × feste Matrix `0x53dc1430` (`setting+0x52`)
3. × **Sensor-CCM** (`set+0x00`, `0x53dc2b58`, für alle Looks gleich)
4. Sättigung: Stufe `setting+0x58` → Faktor aus `0x53dc140c`
   (17 Werte, /128: 0,5× … 2,0×) × Stärke `setting+0x5a`
5. Zeilensummen korrigiert (Grau bleibt grau), Clamp ±0x7FF → Register

Bei den Indizes 13, 14 und 16 setzt der Code die Gains auf `0xAAA` (≈ ⅓ je
Kanal): gleichgewichtete Mischung, also Monochrom.

### Nachtrag: die vollständige Rechnung (Ghidra, 2026-10-12)

```
M = A[Sättigung] × Look-Matrix × Sensor-CCM          (0x536deddc, 0x536dee50: out = P3 × P4)
    (× sRGB→Adobe-RGB 0x53dc1430 davor, wenn Farbraum Adobe RGB, setting+0x52)
Gain: Nebendiagonale skaliert, Diagonale so, dass jede Zeile exakt aufgeht
```

- **A[Sättigung]**: neun Matrizen pro Look (`set+8`), gewählt über den
  Sättigungsregler −4…+4 (`setting+0x57` → `0x53dc1400`). A[4] ist Identität;
  alle Farb-Looks teilen dieselbe Tabelle `0x55018fe8`, Monochrom-Looks haben
  nur Identität, HDR-Variante 24 eine Luma-Matrix.
- **Farbton (Hue)**: wirkt nicht hier, sondern in YCbCrMatrixTuning
  (`0x536e4720`): `YCbCr = H[Farbton] × RGB→YCbCr`, H eine Drehung in Cb/Cr
  um ≈ 2,5° pro Stufe, nach der Tonkurve. Tabelle über `0x55018e4c[Look]`.
- Zeilensummen von Look × Sensor sind bei allen Looks schon exakt 1; die
  Normierung ändert dort nichts.

Der Editor rechnet beides mit (Regler „Sättigung" und „Farbton (Hue)",
vorbelegt aus dem DNG). Messung an `MR021783` (Negative Film, Sättigung +2):
Sättigungslücke Kamera/Editor 1,39 → 1,25, mittlerer Fehler 5,79 → 5,52.
Bei Cross Process passt der DNG-Sättigungswert so nicht (Fit wird schlechter);
der Editor setzt ihn dort auf 0.

Standard gegen Vivid bei Rot: Am Monitor-Farbkreis (MR024507/08) war Rot in
Standard viel reiner als vorhergesagt, Vivid passte. An einem echten roten
Objekt bei Tageslicht (MR024518/19) verschwindet der Unterschied: Kamera
Standard (129, 22, 14) / Vivid (136, 22, 12), gleicher Farbton, Vivid minimal
gesättigter und heller. Der Monitor-Effekt kommt von Rot außerhalb des
Kamera-Farbraums, nicht von einer Look-Stufe.

Was bleibt, ist eine **gemeinsame** Abweichung des Editors bei beiden Looks:
Rot gut 3–4° zu orange und etwas zu wenig gesättigt. Vermutete Ursache: Die
Kamera wählt oder interpoliert die Sensor-CCM nach Weißabgleich, der Editor
nutzt eine feste. Nicht untersucht.

## 4. Index → Bildstil

`ConvertToCommonPropertyImageControl` (`0x53289058`) ist eine Sprungtabelle
UI-Wert 1…12 → interner Index:

| Bildstil | Index | Look-Matrix | Datei-Offset |
|---|---:|---|---:|
| Standard | 11 | `0x53dc2754` | 14.495.684 |
| Vivid | 12 | `0x53dc26dc` | 14.495.564 |
| MonotoneGR | 13 | `0x53dc2a94` (Identität) | 14.496.516 |
| SoftMonotone | 14 | `0x53dc27f0` (Identität) | 14.495.840 |
| HardMonotone | 15 | `0x53dc2698` (Rang 1 = S/W mit Filter) | 14.495.496 |
| HighContrastBW | 16 | `0x53dc2458` (Identität) | 14.494.920 |
| PositiveFilm | 17 | `0x53dc272c` | 14.495.644 |
| BleechBypass_GR | 18 | `0x53dc2474` | 14.494.948 |
| Retro | 19 | `0x53dc2818` | 14.495.880 |
| HdrCuisine | 20 | `0x53dc2754` (= Standard) | 14.495.684 |
| CrossProcess | 21 | `0x53dc2754` (= Standard) | 14.495.684 |
| NegativeFilm | 22 → 25 | `0x53dc2ad0` | 14.496.576 |

**Belegt:** Index 14 = SoftMonotone und 20 = HdrCuisine (Asserts bei
`0x531acac8`/`0x531acafc` vergleichen genau diese Werte). Index 13/14/16
monochrom (Code, siehe oben). **Abgeleitet:** die übrigen Namen aus der
Reihenfolge des Namens-Pools — konsistent mit allen drei Belegen und mit dem
Charakter der Matrizen (Vivid sättigt, Bleach Bypass/Retro entsättigen,
HardMonotone ist eine Rang-1-Matrix).

Indizes 0–10 und 23–25 gehören zu anderen Modi (nicht zugeordnet).

### Gegenprobe an echten Aufnahmen

Jedes DNG der GR III enthält ein von der Kamera gerendertes JPEG und in der
Makernote den Bildstil. Rendert man das RAW mit Sensor-CCM und jeder der 12
Look-Matrizen (Tonkurve jeweils aus dem JPEG gefittet) und vergleicht nur die
Farbe (Abweichung vom Grau, in 8-Bit-Stufen), gewinnt:

| Aufnahme-Stil | Bestes Ergebnis | Zweitbestes |
|---|---|---|
| Positive Film (2 Fotos) | **PositiveFilm** 5,7 | Standard-Matrix 6,0 |
| Negative Film | **NegativeFilm** 4,3 | PositiveFilm 6,4 |
| Negative Film | Standard-Matrix 3,75 | NegativeFilm 3,77 |
| Standard | **Standard**-Matrix 5,1 | Vivid 5,9 |
| Bleach Bypass (2 Fotos) | Monochrom 6,4 | BleechBypass_GR 7,1 |

Die Zuordnung für Positive und Negative Film ist damit auch empirisch
gestützt. Bei Bleach Bypass entsättigt die Kamera stärker, als die Matrix
allein erklärt — dort wirken weitere Tabellen (Sättigung/MultiAxial). Ein
Standard-Foto mit abweichendem Dateischema (`MRI…`) passt besser zu
PositiveFilm; offen. Werkzeug: [`tools/look-editor/`](editor-de.md).

## 5. Korrektur früherer Annahmen

- `CalcTable` bei `0x536e93ac` und die „Auswahlfunktion" `0x536e955c` aus
  colour-pipeline-data.md gehören zu
  **ToneControlTuning**. `0x536e955c` baut ein Histogramm und sucht
  Perzentile — adaptive Schatten-/DR-Korrektur, nicht der Bildstil.
- Die Matrix `0x53dc272c` („farbverschiebend: Grün wandert in Rot und Blau")
  ist die von **Positive Film**.

## 6. Was offen ist

- **Tonkurve:** gelöst, siehe [tone-curves.md](tone-curves.md). Die Kurve
  wird aus 15 Basiskurven und den Reglern berechnet, nicht pro Look gewählt.
  Bleach Bypass und HDR haben Varianten mit eigenen Parametersätzen
  (Tabellenindex 22/23 bzw. 24). Offen bleiben MultiAxial und die
  Tonungs-Tabellen.
- **Geteilte Matrizen:** Standard, HdrCuisine und CrossProcess teilen sich
  eine Matrix. Ein eigener Look für nur einen davon braucht einen freien Slot
  und einen umgebogenen Zeiger im ColorMatrix-Satz (beides in `.data`).
- **Wiederverpacken der `.bin`**: gelöst, siehe
  [firmware-packing.md](firmware-packing.md). Vor jedem Flash bleibt der
  Recovery-Weg zu klären ([firmware-update-security.md](firmware-update-security.md)).
- Statische Analyse: Die Matrizen sind belegt *geladen*; wie sichtbar eine
  Änderung im Bild ist, zeigt erst ein Test.
