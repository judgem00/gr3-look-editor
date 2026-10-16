# Messprotokoll: die Bildpipeline vermessen

Die GR III kann bereits aufgenommene DNGs **in der Kamera** zu JPEG entwickeln
und dabei die komplette einstellbare Bildpipeline anwenden. Damit lässt sich
die Pipeline vermessen, statt sie im Maschinencode zu erraten — ohne jedes
Risiko für die Kamera.

**Der eigentliche Gewinn:** Aus der Firmware haben wir *Kandidaten* — 29
Farbmatrizen, Basistabellen, die `CalcTable`-Formel (siehe
[colour-pipeline-data.md](colour-pipeline-data.md)). Was fehlt, ist die
Zuordnung. Eine gemessene Transformation wählt aus den Kandidaten aus: Passt
eine gemessene Matrix zu einer gefundenen, ist die Zuordnung bewiesen.

## Wichtig: dasselbe DNG mehrfach entwickeln

Der Kern des Verfahrens ist, dass **ein einziges DNG** mehrfach in der Kamera
entwickelt wird — einmal je Einstellung. Dann ist die Einstellung die einzige
Variable, und die Differenz zwischen zwei JPEGs *ist* die Wirkung der
Einstellung. Neu fotografieren zwischendurch macht die Messung wertlos.

## Die Aufnahme

Eine Szene, die den Farbraum abdeckt:

- **Ideal:** eine Farbkarte (ColorChecker o. ä.), gleichmäßig beleuchtet
- **Sonst:** eine bunte Szene mit Hauttönen, Grün, kräftigem Rot und Blau,
  neutralen Grauflächen und einem Verlauf von tiefem Schatten bis knapp unter
  Weiß

Zwei Bedingungen:

1. **Nichts darf clippen.** Kein Kanal an der oberen Grenze — geclippte Werte
   zerstören die Messung im Lichterbereich. Lieber leicht unterbelichten.
2. **Der Tonwertbereich soll ausgenutzt sein.** Ein flaches, graues Bild
   liefert zu wenig Stützstellen.

Zwei oder drei verschiedene Szenen sind besser als eine — die zweite
validiert, was die erste ergibt.

## Kameraeinstellungen bei der Entwicklung — kritisch

Diese müssen über **alle** Entwicklungen identisch sein, sonst misst man sie
mit statt des Bildstils:

| Einstellung | Wert | Warum |
|---|---|---|
| Weißabgleich | **fest** (z. B. Tageslicht), nicht Auto | Auto-WB variiert die Basis pro Bild |
| Dynamikbereichskorrektur / Highlight-Korrektur | **aus** | verändert die Tonkurve unkontrolliert |
| Schattenkorrektur | **aus** | dito |
| Peripherie-Beleuchtungskorrektur (Vignettierung) | **aus** | ortsabhängig, stört die globale Messung |
| Verzeichnungskorrektur | **aus** | verschiebt Pixel, zerstört die Paarbildung |
| Rauschunterdrückung | **niedrigste Stufe**, konstant | lokal, nicht global modellierbar |
| Schärfe | **neutral**, konstant | lokal |
| JPEG-Qualität | **höchste** | weniger Kompressionsartefakte |
| Bildgröße | **konstant** | für die Pixelzuordnung |

## Serie A — die Bildstile

Dasselbe DNG, alle Parameter des jeweiligen Stils auf **Standardwert**:

Standard · Vivid · Positive Film · Negative Film · Retro · HDR-Ton ·
Bleach Bypass · Cross Process · Monochrome · Soft Monochrome ·
Hard Monochrome · Hi-Contrast B&W

## Serie B — der Parameter-Sweep (der wertvollste Teil)

Bildstil **Standard**, jeweils **nur ein** Parameter verändert, alle anderen
neutral:

| Parameter | Stufen |
|---|---|
| Sättigung | −4, −2, 0, +2, +4 |
| Kontrast | −4, −2, 0, +2, +4 |
| Kontrast (Lichter) | −4, 0, +4 |
| Kontrast (Schatten) | −4, 0, +4 |
| Farbton | −4, 0, +4 |

Warum das der wertvollste Teil ist: Aus der Firmware kennen wir die Formel

```
Ergebnis = Basis + (Basistabelle[i] - 4096) * Stärke / 1024
```

Wenn die Stärke bekannt variiert wird, lässt sich die **Basistabelle aus den
Messwerten rekonstruieren** — und direkt mit der im Image gefundenen
vergleichen. Das ist die sauberste Bestätigung, die wir bekommen können.

## Minimalset, falls das zu viel Arbeit ist

Sechs Entwicklungen desselben DNG reichen für den ersten Durchgang:

1. Standard (neutral)
2. Vivid (neutral)
3. Positive Film (neutral)
4. Standard, Sättigung −4
5. Standard, Sättigung +4
6. Monochrome (neutral)

Damit lässt sich das Verfahren validieren, bevor mehr Aufwand hineingeht.

## Was geliefert werden sollte

- die DNG-Datei(en)
- alle daraus entwickelten JPEGs
- keine Umbenennung nötig: Ricoh legt die Einstellungen in den EXIF-MakerNotes
  ab, die lassen sich auslesen (`exiftool`). Falls das für einzelne Parameter
  nicht klappt, hilft eine kurze Liste „Dateiname → Einstellung".

## Was daraus wird

1. DNG entwickeln (linear, ohne Farbkorrektur) → Referenz im Kameraraum
2. JPEG laden → Ergebnis in sRGB
3. Pixelpaare kachelweise mitteln (unterdrückt Rauschen und Schärfung)
4. Transformation schätzen: Weißpunkt → 3×3-Matrix → Tonkurve je Kanal →
   Residuen
5. Ergebnis gegen die Firmware-Kandidaten prüfen

## Grenzen des Verfahrens — vorab benannt

- **Demosaicing** unterscheidet sich zwischen Kamera und unserer Entwicklung.
  Das betrifft Detailwiedergabe, nicht die globale Farb- und Tonabbildung —
  durch Mittelung über Kacheln fällt es weitgehend heraus.
- **Lokale Operationen** (Rauschunterdrückung, Schärfung, Klarheit) lassen
  sich so nicht sauber messen. Deshalb stehen sie im Protokoll auf konstant.
- **JPEG-Kompression** fügt Rauschen hinzu; Mittelung über Kacheln dämpft es.
- Was gemessen wird, ist die **Gesamtabbildung** RAW → JPEG. Sie in die
  einzelnen Pipeline-Stufen zu zerlegen, gelingt nur so weit, wie die
  Firmware-Kandidaten dabei helfen.
