# Look Editor

Bearbeitet Farbmatrizen und Tonkurven der GR-III-Bildstile mit Live-Vorschau
auf einem GR-III-DNG und schreibt eine flashbare Update-Datei.

```sh
./run.sh --update fwdc239b.bin                 # öffnet http://127.0.0.1:8765/
./run.sh --update fwdc239b.bin --dng ~/Pictures/…/MR020978.DNG --port 8765
```

Beim ersten Start legt `run.sh` eine virtuelle Umgebung an (rawpy, numpy,
Flask, Pillow, Unicorn). `exiftool` wird genutzt, wenn vorhanden, um Bildstil
und Regler der Aufnahme aus der Makernote zu lesen.

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
  RTOS `0x536e7b64`), ausgeführt im CPU-Emulator Unicorn auf den (bearbeiteten)
  Basiskurven, mit Key/Kontrast/Lichter/Schatten
  ([docs/tone-curves.md](tone-curves.md)).
- Eingangsskalierung und Gain werden pro DNG am eingebetteten Kamera-JPEG
  gefittet; die **Rest-Korrektur** (abschaltbar) fängt die bildabhängige
  Schattenkorrektur der Kamera auf.

Ansichten: **Bearbeitet**, **Vorher/Nachher** (Firmware-Original gegen
Bearbeitung, Farbe und Tonkurve), **Vergleich Kamera**, **Kamera-JPEG**.

## Bearbeiten

Aufbau der Oberfläche: oben rechts die Bildstil-Auswahl (Namen wie im
Kameramenü), darunter die Tabs **Farbe**, **Tonkurve**, **Cross Process** und
**Export**. Fachdetails (Matrixwerte, Kontraststufen, Kurven kopieren) liegen
jeweils unter „Erweitert". Die Kamera-Einstellungen der Aufnahme stehen in der
aufklappbaren Leiste unter dem Bild. Der Tab Cross Process schaltet die
Vorschau auf diesen Bildstil um.

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

Schreibt eine **Kopie** von `unpackedFW` (unter `work/<fw>/unpackedFW-looks`),
eine JSON mit Matrizen und Kurvenpunkten und die gepackte Update-Datei
`FWDC239B.BIN` mit neu berechneten Prüfsummen
([docs/firmware-packing.md](firmware-packing.md)). Das Original wird
nie überschrieben. Nach dem Export bietet die Oberfläche die Dateien zum
**Download** an; die Update-Datei behält ihren Namen `FWDC239B.BIN`, den die
Kamera im Wurzelverzeichnis der SD-Karte erwartet. Ändert man danach weiter,
verschwindet das Angebot, bis neu exportiert wird. Vor einem Flash:
Recovery-Weg klären.

**Preset laden:** Die beim Export geschriebene `*.looks.json` lässt sich über
„Preset laden (JSON)" oder per Drag & Drop auf die Vorschau wieder einlesen.
Sie ersetzt alle aktuellen Änderungen: Matrizen, Tonkurve,
Kontrastfamilien, Kanalkurven und Look-Übernahme. Von dort lässt sich
weiterarbeiten und neu exportieren.

Standard, HdrCuisine und CrossProcess teilen sich eine Matrix: eine Änderung
gilt für alle drei.
