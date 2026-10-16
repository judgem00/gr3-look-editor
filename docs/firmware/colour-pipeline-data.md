# Farbdaten der Bildpipeline — Fundstellen

Ergebnis von Phase 2 des [Farbprofil-Plans](custom-looks-plan.md): Welche
Stellschrauben für Farbe liegen als editierbare Daten im Firmware-Image?

Firmware `fwdc239b` (GR III), Datei `assets/fwdc239b/unpackedFW`.
Ladeadresse: `RAM = Datei-Offset + 0x52FEF790`.

## Kurzfassung

Der Satz ist beisammen: **Farbmatrizen, Tonkurven und Parameterstufen liegen
als Festkomma-Daten im Image** und sind damit austauschbar. Was für einen
Farblook gebraucht wird, ist vorhanden.

## 1. Farbmatrizen (3x3 CCM)

**29 Matrizen in einem Block bei Datei-Offset 14.490.784 – 14.496.782**
(rund 6 KB), plus zwei Einzelfunde bei 15.782.296 und 15.784.548.

- Format: **9 × int16, little-endian, Festkomma mit Skala 8192** (= 1,0)
- Zeilensumme jeder Matrix exakt 8192 — dadurch bleibt Neutralgrau neutral
- Abstände überwiegend 20 Bytes (18 Byte Matrix + 2 Byte Ausrichtung), teils
  in Gruppen mit größeren Lücken dazwischen

Beispiele:

```
@ 14.495.624   +1.5273  -0.5312  +0.0039      kräftig sättigend
               -0.2109  +1.5664  -0.3555
               +0.0352  -0.4141  +1.3789

@ 14.495.644   +0.9773  +0.2852  -0.2625      farbverschiebend:
               -0.0189  +1.1692  -0.1503      Grün wandert in Rot und Blau
               -0.0651  +0.3711  +0.6940

@ 14.495.702   +0.8276  +0.1455  +0.0269      milde Farbkorrektur
               +0.0756  +0.8962  +0.0282
               +0.0770  +0.1436  +0.7794
```

Die zweite ist der interessante Typ: keine reine Sättigungsänderung, sondern
eine Farbdrehung — also genau das, was den Charakter eines Looks ausmacht.

Werkzeug: [`tools/find-matrices.py`](../../../gr3x-fw-hack/tools/find-matrices.py)

```sh
tools/find-matrices.py assets/fwdc239b/unpackedFW \
    --width 2 --scale 8192 --nontrivial --tolerance 10000
```

## 2. Neutral vorbelegtes Matrix-Array

Bei **14.498.404 – 14.500.492** liegen **60 Identitätsmatrizen** lückenlos im
36-Byte-Raster (9 × int32, Skala 1024) — ein Array mit 60 Slots, sämtlich
neutral.

Deutung (unbestätigt): entweder zur Laufzeit gefüllte Slots, oder ein Bereich,
der pro Kamera aus Werkskalibrierungsdaten beschrieben wird. Für eigene
Profile wäre das ein interessanter Angriffspunkt, weil dort ohnehin Matrizen
erwartet werden.

## 3. Tonkurven

| Datei-Offset | Inhalt |
|---:|---|
| 14.558.190 | 14-Bit-Tonkurve, 16.386 Einträge, hebt Schatten an |
| ~14,55–14,60 MB | drei Tabellen à 2048 Einträge, voller 16-Bit-Bereich |
| 15,81–15,92 MB | Kurvenbank: 50+ Kurven à 1024 Stützstellen, abnehmende Steilheit |

Werkzeug: [`tools/find-luts.py`](../../../gr3x-fw-hack/tools/find-luts.py)

## 4. Parameterstufen

Im Tuning-Block liegen symmetrische Neun-Werte-Folgen, zum Beispiel:

```
-48  -36  -24  -12    0   32   64   96  128
-24  -18  -12   -6    0   16   32   48   64
 -8  -16  -16   -8    8   16   16    8   (…)
```

Neun Stufen um die Null — das entspricht dem Einstellbereich −4…+4 der
Kamera-Bildparameter. Diese Tabellen bilden eine Menüstufe auf einen
Korrekturwert ab.

## 5. Die Tuning-Registry

Bei Datei-Offset **17.454.552** (RAM `0x54094d68`): **30.628 Zeiger auf 1.095
verschiedene Ziele** — die zentrale Verteilstruktur der Bildabstimmung. Darin
unter anderem eine Kurvenfamilie mit sieben abgestuften Kurven.

## 6. Wie aus Daten eine fertige LUT wird (`CalcTable`)

Die entscheidende Mechanik, gefunden im Code bei Datei-Offset 7.314.396
(RAM `0x536e93bc`):

```
movw lr, #0x1f5c
movt lr, #0x53dc          ; Basistabelle = Datei-Offset 14.493.644
add  r6, lr, #512         ; Schleife über 0x200 Bytes = 256 Werte
loop:
  ldrh r3, [lr]           ; Wert aus der Basistabelle
  sub  r3, r3, #4096      ; minus 1,0  (12-Bit-Festkomma)
  mul  r3, r2, r3         ; mal Stärke-Parameter
  asr  r3, r3, #10        ; geteilt durch 1024
  strh r3, [r5, #2]!      ; in die Ziel-LUT schreiben
  cmp  lr, r6
  bne  loop
```

Also:

```
Ergebnis = Basis + (Basistabelle[i] - 4096) * Stärke / 1024
```

Die **Basistabelle bei 14.493.644** ist eine Wirkungskurve: 256 Werte, die von
8160 (≈ 2,0×) monoton auf 4096 (= 1,0, neutral) fallen; 125 der 256 Einträge
sind exakt neutral. Sie bestimmt, *wie stark* der Effekt bei welchem
Eingangswert greift. Der Stärke-Parameter kommt aus der Menüeinstellung.

**Konsequenz für eigene Profile:** Die fertigen LUTs stehen nicht im Image —
sie werden erzeugt. Anzusetzen ist an den **Basistabellen und Matrizen**, nicht
an einem Endergebnis. Das ist eine gute Nachricht: Eine geänderte Basistabelle
verändert den Charakter des Effekts, während der Nutzer die Stärke weiterhin
im Menü regelt.

## 7. Ein Kommando-Interface mit Klarnamen

Die Namen aus dem String-Pool (`StillImageControlNo0ParamVividSaturation` …)
sind keine Struktur-Feldnamen, sondern **Kommandos eines Interpreters**. Der
Code bei Datei-Offset 3.504.636 vergleicht eingehende Zeichenketten der Reihe
nach gegen diese Namen und springt per Tail-Call in eine Funktion je Name
(Sprungtabelle bei 3.529.560). Jede dieser Funktionen vergleicht dann das
Argument gegen weitere Namen.

Der Argument-Pool bei Datei-Offset 11.745.088 enthält die Bildstile im
Klartext:

```
Standard  Vivid  MonotoneGR  SoftMonotone  HardMonotone  HighContrastBW
NegativeFilm  PositiveFilm  BleechBypass_GR  Retro  HdrCuisine  CrossProcess
Sepia  Red  Blue  Purple  Cool  Warm        (Tonungen)
P1  P2  P3  P4                              (Cross-Process-Varianten)
No0  No1  No2                               (Slots)
Strong  Weak  Low  Mid
```

Die Firmware nimmt also Bildstile und Parameter **per Namen** entgegen —
etwa in der Art `StillImageControlNo0 Vivid`.

**Erreichbarkeit — nachgeprüft, Ergebnis negativ.** Die naheliegende
Vermutung war der `term-io`-Kanal aus dem `shared_commem`-Knoten des Device
Tree. Sie hält nicht:

- `libcmfwk.so` — die einzige Bibliothek der Linux-Seite, die
  `shared_commem` überhaupt anfasst — liest **nur** `buf`, `movie-rec`,
  `rd-pointer`, `wt-pointer`, `string`, `ribery`, `audio`, `raw` und `yuv`.
  **`term-io` ist nicht dabei.** Der Terminal-Kanal ist im Device Tree
  definiert, wird von Linux aber nicht bedient.
- Der Interpreter hat keine `bl`-Aufrufer; er wird über einen Funktionszeiger
  erreicht. Eine Kommando-Tabelle der Form {Name, Handler} für die Bildstile
  ließ sich im Image nicht finden (wohl aber solche Tabellen für Menüs, etwa
  die ISO-Liste bei 11.891.200).

Die Umgebung des Interpreters (`FactoryFwUpdateController`,
`TouchPanelTestController`, `Factory Fw Update`) spricht dafür, dass er zu
einem **Werks-/Servicemodus** gehört, der nicht über einen normalen Kanal
offensteht. Ein Zugang wäre damit selbst ein eigenes Forschungsthema — und
kein kürzerer Weg als der Firmware-Patch.

> **Korrektur:** `CalcTable` und die auswählende Funktion gehören zu
> `ToneControlTuning` (adaptive Schatten-/DR-Korrektur), nicht zum Bildstil.
> Die tatsächliche Zuordnung Bildstil → Daten steht in
> [look-mapping.md](../../../gr3x-fw-hack/docs/look-mapping.md).

## 8. Aufrufkette der Tabellenberechnung

| Ebene | Datei-Offset | Rolle |
|---|---:|---|
| Tabellenbau (`CalcTable`) | 7.314.460 | interpoliert Basistabelle × Stärke |
| einziger Aufrufer | 7.314.892 | wählt Basistabellen anhand von Struktur-Bytes |
| darüber | — | kein `bl`-Aufrufer: Aufruf über Funktionszeiger |

Die auswählende Funktion liest Parameter aus einer Struktur (`[r8, #83]`,
`[r8, #84]`, `[r8, #138]`, `[r8, #139]`) und lädt je nach Wert eine von
mehreren Tabellen im Matrix-Block. Welche Bedeutung die Struktur-Offsets
haben, ist offen — hier bricht die statische Spur ab.

Werkzeug: [`tools/find-callers.py`](../../../gr3x-fw-hack/tools/find-callers.py)

## Was fehlt

- **`m_MultiAxialTuning`** ist nicht eindeutig lokalisiert. Die Neun-Werte-
  Folgen sind Parameterstufen, nicht die mehrachsige Farbkorrektur selbst.
  Für einen Look wäre sie die Kür, nicht die Pflicht — Matrix plus Tonkurve
  plus Sättigung trägt bereits ein Profil.
- **Die Zuordnung Look → Daten** ist weiterhin nicht bewiesen. Wir wissen, wo
  die Farbmatrizen liegen, aber nicht, welche zu „Positive Film" gehört.
- Ob die Werte im Image tatsächlich benutzt werden oder pro Kamera aus
  Kalibrierungsdaten überschrieben werden (siehe Abschnitt 2).
- Ein Diff-Weg zur Zuordnung steht nicht zur Verfügung: ältere GR-III-
  Firmwares sind nicht mehr auffindbar, und eine Monochrome-Variante gibt es
  erst ab der GR IV. Bleibt die Codeanalyse aufwärts: wer ruft `CalcTable`
  mit welcher Basistabelle, und woher kommt die Auswahl?

## Nächster sinnvoller Schritt

Die Zuordnung angehen: mit dem bekannten Bias die Zugriffe auf den
Matrix-Block verfolgen, um von einem Bildstil zu seiner Matrix zu kommen.
Alternativ empirisch über einen Vergleich zweier Firmware-Versionen oder mit
der GR III Monochrome, die keine Farbstile hat.
