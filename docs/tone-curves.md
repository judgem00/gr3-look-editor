# Tonkurven der GR III

Wie die Kamera ihre Tonkurve bildet, wo die Daten liegen und wie der
[Look Editor](editor-de.md) sie bearbeitet. Firmware
`fwdc239b`, Analyse mit REA/Ghidra und Capstone.

## Kurzfassung

- Die Tonkurve wird **pro Aufnahme berechnet**, nicht pro Bildstil
  nachgeschlagen. `GammaTuning` (RTOS `0x536e7b64`) mischt sie aus
  **15 Basiskurven** anhand von vier Reglern: Key, Kontrast, Kontrast Lichter,
  Kontrast Schatten.
- Die Bildstile unterscheiden sich in der Tonkurve nur über diese Reglerwerte;
  einen Look-Index nimmt GammaTuning nicht entgegen.
- Der Editor führt **den Originalcode der Kamera** in einem ARM-Interpreter
  aus (`core/src/arm.rs`; geprüft gegen 7261 Kurven aus dem CPU-Emulator
  Unicorn). Bearbeitete Basiskurven laufen durch dieselbe Arithmetik wie in der
  Kamera; die Vorschau ist bei gleichen Reglern bitgenau die Kamerakurve.

## Datenlage

| Was | Wo | Format |
|---|---|---|
| 3 Kurvenfamilien | `.data` `0x550204d4[0..2]` | Zeiger auf je 5 Kurvenzeiger |
| 15 Basiskurven | z. B. `0x53ddd394`, `0x53dcf7f0` … | 256 × u16, 0…~16300 |
| Key-Tabelle | `0x538f34e0` | 9 × int: 512 … 256 … 128 |
| Glättung Übergang | `0x53ddd594` | u16-Gewichte, /2048 |

Familie 0 = hoher, 1 = normaler, 2 = niedriger Kontrast. Die 5 Kurven einer
Familie gehören zu Weißpunkt-Bändern (Knie bei 0x80, 0x100, 0x200, 0x400,
0x800 auf der 12-Bit-Eingangsskala).

## Der Algorithmus (`0x536e7b64`)

Eingaben aus dem Setting (befüllt von `0x536e84d0`): Key−4, Kontrast−4,
4−Schatten, Lichter−4, ein Flag und ein Laufzeit-Gain (`setting+100`).

1. **Knie:** `Gain × Key-Tabelle[Key]` ergibt den Kniepunkt auf der
   12-Bit-Eingangsskala und damit die zwei Weißpunkt-Bänder.
2. **Kontrast je Bereich:** Für Lichter, Mitte und Schatten getrennt wird
   `2·Kontrast + Bereichsregler` (−12…+12) auf eine Mischung zweier
   benachbarter Familien abgebildet.
3. **Bänder mischen** so, dass der Wert am Knie **fest bei `0x1d80`** liegt —
   die Kamera verankert ihre Kurve dort.
4. **256 Stützstellen** (Eingang in 16er-Schritten): oberhalb des Knies die
   Lichter-, unterhalb die Schatten-Mischung, in den Übergängen mit der
   Glättungstabelle zur Mitte überblendet; Ausgabe `× 0x4000 / 0x3f02`.

Folge für Bearbeitungen: Eine globale Anhebung aller Basiskurven wird durch
Schritt 3 teilweise wieder ausgeglichen. Deshalb zeigt der Editor die
**wirksame** Kurve aus dem Emulator, nicht die naive Erwartung.

## Bildstil-Regler in der Pipeline

`FUN_536db7d8` bildet die Kamera-Einstellungen pro Look auf die
Tuning-Parameter ab (Monochrom-Looks: Sättigung/Farbton fest in Mitte):

| Kamera-Feld | Regler | Tuning → Setting |
|---|---|---|
| `+0x77d` | Sättigung | `+0x1e0` → `+0x57` |
| `+0x77e` | Farbton | `+0x1e2` → `+0x5c` |
| `+0x77f` | Key | `+0x1e4` → `+0x68` |
| `+0x780` | Kontrast | `+0x1e6` → `+0x6c` |
| `+0x781` | Kontrast Lichter | `+0x1e8` → `+0x74` |
| `+0x782` | Kontrast Schatten | `+0x1ea` → `+0x70` |
| `+0x785…788` | Filter/Tonung je Look | `+0x1f0` → `+0x60` |

Im DNG stehen die Werte in Makernote-Tag `0x0247` (int16, −4…+4,
`0x8000` = nicht belegt). Die Reihenfolge Sättigung, Farbton, Key, Kontrast,
Lichter, Schatten, Schärfe ist **abgeleitet** (Speicherlayout und
Parameterreihenfolge der Firmware), nicht am Schreibcode belegt.

## Vorschau-Treue

Zwei Größen hängen an der Aufnahme und werden am Kamera-JPEG gefittet: die
Eingangsskalierung (Sensor → 12-Bit-Eingang) und der Gain. Was keine globale
Kurve abbildet — vor allem die bildabhängige **Schattenkorrektur**
(`ToneControlTuning`, Makernote „ShadowCorrection") — fängt eine abschaltbare
Rest-Korrektur aus dem JPEG auf.

| Aufnahme | nur Kamerakurve | mit Rest-Korrektur | freie Kurve (Referenz) |
|---|---|---|---|
| Standard, alle Regler 0 | 6,9 | 5,0 | 4,9 |
| Negative Film, eigene Regler | 9,9 | 7,8 | 7,9 |

(mittlere Abweichung zum Kamera-JPEG in 8-Bit-Stufen)

Die Kurve selbst ist exakt — der Emulator führt den Kameracode aus, und die
Kurve auf einem exportierten Image ist bitgleich mit der Vorschau. Die
Restabweichung kommt aus lokaler Verarbeitung und den nicht emulierten
Stufen danach.

## Kanalkurven (R, G, B)

GammaTuning legt **vier Ausgabetabellen** an (Objekt `+0x2a08` R, `+0x2c08` G,
`+0x2e08` B, `+0x3008` Luma). Normalerweise kopiert `0x536e8660` die
berechnete Kurve in alle vier. Ist das Filter-Flag gesetzt (`setting+0x9b`,
aus der Kamera-Einstellung `+0x38a98`), entsteht jeder Kanal über eine
Kanalkurve:

```
Kanal[i] = C(Basis[i])      C über Basis >> 6 interpoliert (64er-Schritte)
Luma[i]  = (77 R + 150 G + 29 B) >> 8
```

Der Filter ist **Cross Process**. Sein Koordinator (Konstruktor `0x536eea90`,
Auswahl `0x536ef140`) wählt je Variante drei der **12 Kanalkurven**
(`.data 0x55037604[0..11]` → `0x53efb0e4…0x53efc728`, je 256 × u16):

| Variante | R | G | B |
|---|---|---|---|
| Zufall | 0–2, Reihenfolge pro Aufnahme zufällig (`0x53efb910/918/920`) | | |
| Preset 1 | 3 | 4 | 5 |
| Preset 2 | 6 | 7 | 8 |
| Preset 3 | 9 | 10 | 11 |

Bei den Presets übernimmt der Koordinator die aktuellen Look-Einstellungen
und setzt nur die Kanalkurven.

**Menü-Zuordnung, an der Kamera belegt (2026-10-12):** Cross Process hat eine
Einstellung „Farbton" 1–3; sie steht als letzter Wert in Makernote-Tag
`0x0247`. Eine Testfirmware mit Markerkurven (Preset 1 rot, 2 grün, 3 blau,
Zufall als Negativ) ergab für Farbton 1, 2, 3 genau rot, grün, blau:
**Farbton n = Preset n.** Der Zufallssatz trat nicht auf; über Farbton 1–3 ist
er nicht erreichbar. Damit lassen sich drei RGB-Kurvensätze flashen und in der
Kamera umschalten.

Offen: Preset 1 hebt Blau in den Tiefen an (Kurve 5 beginnt bei 1920); in
Original-Aufnahmen mit Farbton 1 waren die Schwärzen aber nicht blau. Die
Vorschau der Tiefen von Farbton 1 ist daher nicht gesichert. Andere Bildstile nutzen die Kanalkurven nicht — dafür wäre ein
Code-Patch nötig.

Der Editor bildet `0x536e8660` in `chan::compose` (`core/src/chan.rs`) nach; gegen den
emulierten Originalcode ist das für alle vier Sätze bitgleich geprüft.

## Cross Process als Träger für RGB-Kurven

Weil nur Cross Process Kanalkurven anwendet, kann es als „Look mit
RGB-Kurven" dienen:

1. **Look-Daten übernehmen.** Der Setting-Befüller wählt die Look-Daten über
   drei Tabellen in `.data` (`0x55018d94`, `0x55018cf0`, `0x55018e4c`), indiziert
   mit dem Look. Zeigt Eintrag 21 (Cross Process) auf dieselben Daten wie
   Eintrag 25 (Negative Film), rechnet Cross Process mit Matrix und Look-Daten
   von Negative Film. Standard und HDR, die bisher denselben Datensatz wie
   Cross Process nutzen, bleiben unberührt. Für Negative Film ändert sich nur
   ein Zeiger.
2. **Tonkurve.** Sie hängt an den Reglern, nicht am Look: In der Kamera bei
   Cross Process dieselben Werte wie bei Negative Film einstellen und eine
   Preset-Variante wählen (die übernimmt die aktuellen Einstellungen; „Zufall"
   setzt Look und Regler selbst).
3. **RGB linear.** `C[k] = 64·k` reicht die Tonkurve durch. Die Kamera liest
   für Eingaben ≥ 0xff·64 nur den letzten Eintrag; mit `C[255] = 16383` bleibt
   Weiß Weiß und die Abweichung unter einer 8-Bit-Stufe (nur oberste 0,8 %).

Im Editor ist Cross Process mit übernommenen Negative-Film-Daten und linearen
Kanalkurven pixelgleich mit Negative Film; von dort aus lassen sich die
RGB-Kurven bearbeiten. Nicht belegt: ob andere Module den Look-Index
(21 statt 25) für eigene Fallunterscheidungen nutzen — `FUN_536db7d8`
behandelt 21 anders als 22 (Cross-Process-Tonung `+0x797` statt
`+0x795`).

## Farbräder

Die Farbräder des Editors sind eine Bedienschicht über den Kanalkurven
(`chan::grade` in `core/src/chan.rs`). Ein Rad mit Farbton h, Stärke a und Luminanz l
verschiebt Kanal c (φ = 0°, 120°, 240° für R, G, B) um

```
w(x) · (0,2 · a · cos(h − φ_c) + 0,2 · l)
```

mit den Gewichten Schatten (1 − x)², Mitteltöne 4x(1 − x), Lichter x²,
Global 1 über dem berechneten Tonwert x. Balance verschiebt x → x^(2^−b).
Die Kosinusterme summieren sich zu null, eine Tönung lässt den Kanalmittelwert
also unverändert. Das Ergebnis wird auf 0…0x3fff begrenzt. Monotonie wird nicht
erzwungen, weil schon mehrere Original-Kanalkurven kleine Dellen haben.

Selektive Farbänderungen (HSL je Farbton) sind so nicht möglich. Dafür braucht
es eine farbtonabhängige Stufe in der Firmware, Kandidat ist MultiAxial
(noch nicht untersucht).

## Was offen ist

- Die Basiskurven werden auch von anderen Pfaden gelesen (Zeigertabelle in
  `.data` ab `0x55020470`, z. B. `0x536e82d0` für Sonderfälle). Eine
  Bearbeitung wirkt dort mit.
- Die Schattenkorrektur (`0x536e955c`, Histogramm) ist nicht emuliert.
- Retro- und Bleach-Bypass-Tonungen laufen über eigene Tabellen
  (`setting+8` → `+0x34…+0x40`); nicht bearbeitbar.
- Was die Einstellung `+0x38a98` im Kameramenü ist (Werte 1–11, Zuordnung
  über `0x53dbd19c`).
