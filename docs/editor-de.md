# Look Editor

Bearbeitet Farbmatrizen und Tonkurven der GR-III-Bildstile mit Live-Vorschau
auf einem GR-III-DNG und schreibt eine flashbare Update-Datei.

Läuft komplett im Browser: Ricohs Update-Datei `fwdc239b.bin` hineinziehen,
dann ein DNG. Der Rust-Kern (`core/`, als WebAssembly) liest Firmware und DNG,
auch Bildstil und Regler der Aufnahme aus der Makernote. Nichts wird
hochgeladen.

## Was die Vorschau rechnet

```
Kamera-RGB -> WB (AsShotNeutral) -> Sensor-CCM -> Look-Matrix -> A[Sättigung] -> (Mono-Mix)
           -> Kamera-Tonkurve (bei Cross Process je Kanal) -> Farbton-Drehung (YCbCr)
           -> (Rest-Korrektur)
```

Sättigung und Farbton (Hue) sind die Bildstil-Regler der Kamera; ihre Matrizen
kommen aus der Firmware ([docs/look-mapping.md](look-mapping.md)).

- **Matrizen** kommen aus dem Firmware-Image
  ([docs/look-mapping.md](look-mapping.md)).
- **Die Tonkurve rechnet der Originalcode der Kamera** (`GammaTuning`,
  RTOS `0x536e7b64`), ausgeführt in einem kleinen ARM-Interpreter
  (`core/src/arm.rs`) auf den (bearbeiteten)
  Basiskurven, mit Key/Kontrast/Lichter/Schatten
  ([docs/tone-curves.md](tone-curves.md)).
- Eingangsskalierung und Gain werden pro DNG am eingebetteten Kamera-JPEG
  gefittet; die **Rest-Korrektur** (abschaltbar) fängt die bildabhängige
  Schattenkorrektur der Kamera auf.

Ansichten: **Bearbeitet**, **Vorher/Nachher** (Firmware-Original gegen
Bearbeitung, Farbe und Tonkurve), **Vergleich Kamera**, **Kamera-JPEG**.

## Bearbeiten

Aufbau der Oberfläche: Der Editor baut einen eigenen Look in den Platz von
**Cross Process**; die übrigen Bildstile bleiben unverändert. Tabs:
**1 Grundlage** (Farben eines vorhandenen Bildstils als Start, werden in eine
eigene Matrix für Cross Process kopiert), **2 Farbe**, **3 Kurven** (Farbton
1–3, RGB-Kurven gemeinsam oder je Kanal, Farbräder; die Kurven starten immer
linear), **4 Export**. Andere Bildstile und die Tonkurve für alle Bildstile
liegen unter „fortgeschritten". Technisch bekommt Cross Process einen eigenen
ColorMatrix-Satz und eine eigene Matrix im freien Bereich `0x53002080…`
(`core/src/looks.rs`, `private_cross`; der Satz ist 21 Wörter groß).

- **Farbmatrix** pro Bildstil, direkt oder über Sättigung/Farbton.
- **Kalibrierung** wie in Lightroom: Farbton und Sättigung der Primärfarben
  Rot, Grün, Blau. Das ist exakt als Matrix darstellbar (`Look = Sättigung ·
  Farbton · Basis · Kalibrierung`): Die Primärfarben werden um die Grauachse
  gedreht bzw. skaliert, dann jede Spalte so skaliert, dass Weiß weiß bleibt.
  Die übrigen Primärfarben behalten ihren Farbton (< 1° Übersprechen).
  Presets speichern die Regler mit (`matrix_edits`).
  Bleach Bypass und HDR haben Varianten mit eigener Matrix.
- **Tonkurve:** Punkte klicken, ziehen, per Doppelklick entfernen. Die Kurve
  wird auf die 15 Basiskurven angewandt (wahlweise nur auf einzelne
  Kontrastfamilien); die Kamera mischt danach wie gewohnt. Sie **gilt für alle
  Bildstile**. Die untere Grafik zeigt die tatsächlich wirksame Kamerakurve.
- **Kanalkurven (R, G, B)** der Cross-Process-Varianten **Farbton 1–3** (die
  Farbton-Einstellung im Kameramenü; intern Preset 1–3) und Zufall (über das
  Menü nicht erreichbar). Beim Laden eines Cross-Process-DNGs wählt der Editor
  den Satz der Aufnahme. Bearbeiten wie die Tonkurve; per Klick einen Kanal oder einen ganzen Satz
  aus einer anderen Variante übernehmen. Sie wirken nur bei Cross Process —
  die Vorschau wendet sie an, wenn dieser Bildstil gewählt ist.
- **Farbräder** (Schatten, Mitteltöne, Lichter, Global, mit Luminanz und
  Balance) wie bei Lightroom-Color-Grading. Sie werden in die R/G/B-Kanalkurven
  des gewählten Cross-Process-Satzes gerechnet und wirken damit wie RGB-Kurven,
  nicht in einem Wahrnehmungsfarbraum. Bei „Zufall" verteilt die Kamera die
  Kurven pro Aufnahme neu, eine Tönung wechselt dann die Farbe. Das Preset
  speichert die Räder mit, sie bleiben nach dem Laden bearbeitbar.
- **Cross Process mit fremdem Look:** Bei Cross Process „Look-Daten übernehmen
  von …" (z. B. Negative Film) und „RGB linear" ergibt einen Look, der wie
  Negative Film rechnet und RGB-Kurven trägt
  ([docs/tone-curves.md](tone-curves.md)).
- **Kamera-Einstellungen** (Key, Kontrast, Lichter, Schatten, Belichtung)
  wirken nur auf die Vorschau; vorbelegt aus dem DNG.

## Export

Baut die Update-Datei `FWDC239B.BIN` mit neu berechneten Prüfsummen
([docs/firmware-packing.md](firmware-packing.md)), prüft sie wie der
Decoder der Kamera und bietet sie zusammen mit einem Preset (`*.looks.json`)
zum **Download** an. Die Update-Datei heißt `FWDC239B.BIN`, wie die Kamera
sie im Wurzelverzeichnis der SD-Karte erwartet. Ändert man danach weiter,
verschwindet das Angebot, bis neu exportiert wird. Vor einem Flash:
Recovery-Weg klären.

**Preset laden:** Die beim Export geschriebene `*.looks.json` lässt sich über
„Preset laden" in der Titelleiste oder per Drag & Drop auf die Vorschau wieder
einlesen; „Beispiel-Preset laden" lädt das mitgelieferte
`editor/static/presets/example.looks.json`. Ein Preset ersetzt alle aktuellen
Änderungen: Regler, Tonkurve, Kontrastfamilien, Kurvenpunkte der Kanalkurven,
Farbräder und Look-Übernahme. Die Basis-Matrix steht nur drin, wenn sie von
Hand eingegeben wurde; sonst kommt sie beim Laden aus der Firmware. Von dort lässt sich
weiterarbeiten und neu exportieren.

Standard und HdrCuisine teilen sich eine Matrix: eine Änderung gilt für
beide. Cross Process hat seine eigene Kopie.
