# Update-Datei packen und Prüfsummen

Wie aus einem geänderten `unpackedFW` wieder eine Update-Datei wird, die die
Kamera annimmt. Firmware `fwdc239b` (GR III, Build 2025-09-25).

Werkzeug: [`tools/fw-pack.py`](../fw-pack.py), Format in
[`tools/fwpack/grfw.py`](../fwpack/grfw.py). Der
[Look Editor](editor-de.md) erzeugt die Datei beim Export
direkt mit.

```sh
tools/fw-pack.py check firmware/fwdc239b.bin
tools/fw-pack.py pack firmware/fwdc239b.bin assets/fwdc239b/unpackedFW-looks \
    -o assets/fwdc239b/FWDC239B.BIN
```

## Kurzfassung

- Der Container ist durch **zwei additive Prüfsummen** gesichert, beide
  „Summe aller Little-Endian-32-Bit-Wörter = 0". Keine Signatur, kein Hash.
- Beide lassen sich neu berechnen. **Neu packen des unveränderten Images
  ergibt byteidentisch die Originaldatei.** Damit ist das Format vollständig
  verstanden.
- Eine geänderte Look-Matrix kostet zwei neu kodierte Frames (+24 KB). Die
  Datei besteht alle Prüfungen des Kamera-Decoders, und der unabhängige
  `gr_unpack` entpackt exakt das geänderte Image.

## Aufbau der Datei

```
0x00000000  Header, 0x80 Byte
              +0x00 "RICOH" / "GR III" / Firmenname
              +0x30 Projektnummer 0x24c (= 588, "KB588")
              +0x38 Version 02 0a 15 1b
              +0x54 Build-Kennung 0x438ff331 (steht auch im RTOS, RAM 0x53002054)
              +0x58 "2025/09/25 11:28:34 JST"
              +0x78 0x024c024c        +0x7c 0xa55a5aa5
0x00000080  komprimierter Strom: 2430 Frames, Endmarke 0x0000
0x01cfbdca  2 Byte Füllung auf 4-Byte-Grenze
0x01cfbdcc  Footer, 0x18 Byte
              +0x00 0x024c024c        +0x04 0xa55a5aa5
              +0x08 FW-Typ 2          +0x0c Stromlänge inkl. Endmarke
              +0x10 entpackte Länge   +0x14 Prüfsumme
```

Frames (Format aus `gr_unpack`): Jeder Frame entpackt zu genau **0x6000
Byte** (eine Ausnahme mit 0x8000, dazu der letzte). 729 sind unkomprimiert,
1701 LZ-komprimiert; Rückverweise (bis 8 KB) reichen über Frame-Grenzen.

## Die Prüfungen der Kamera

`FwDecoder` im RTOS, der Reihe nach:

| Prüfung | Code | Meldung bei Fehler |
|---|---|---|
| Header +0x30 = Projektnummer | `0x5362bd58` | `project number is different!` |
| Header +0x78 = Projektnummer ×2 | `0x5362bdb0` | `header project number is different!` |
| Header +0x7c = `0xa55a5aa5` | `0x5362bdc0` | `header chek data is invalid!` |
| Footer +0x00/+0x04/+0x08 | `0x5362be0c` | `footer … is different/invalid!` |
| **Wortsumme der Datei = 0** | `0x5362bcc8` | `check sum is invalid!` |
| Entpackte Länge = Footer +0x10 | `0x5362c020` | `decode FW size fail…` |
| **Wortsumme des Images = 0** | `0x5362bea8` | `decoded check sum error!` |
| Version nicht älter als installiert | `0x53628574` | `Update Version is older…` |

Ausgleichswerte: Footer +0x14 für die Datei, das **letzte Wort des Images**
für das Image. Die Kamera überspringt die Dateiprüfung, wenn das Feld
`0xffffffff` ist, und die Imageprüfung bei letztem Wort 0, `0xfffffffe` oder
`0xffffffff`. `fw-pack` nutzt das nicht, sondern rechnet korrekt.

Die Versionsprüfung lehnt nur **ältere** Versionen ab (`blo`); dieselbe
Version wird akzeptiert. `fw-pack` übernimmt den Header unverändert.

Das letzte Image-Wort liegt hinter den letzten Ressourcendaten (davor endet
ein JPEG mit `ff d9` genau an `0x38f3af0`). Es gehört zu keiner Partition.

## Wie gepackt wird

Minimal-invasiv: Der Originalstrom wird Frame für Frame übernommen. Neu
kodiert, als unkomprimierter Frame im 0x6000-Raster, werden nur Frames, deren
Ausgabe sich ändert, und Frames, die per Rückverweis aus geänderten Bytes
kopieren. Danach werden Imageprüfsumme, Footer und Dateiprüfsumme neu
berechnet. Bevor etwas geschrieben wird, wird das Ergebnis erneut dekodiert
und verglichen.

## Korrektur früherer Annahmen

[firmware-update-security.md](firmware-update-security.md) beschreibt eine
byteweise Summe, die „auf ein Byte gefaltet" gegen ein Header-Byte geprüft
wird (`CHECKSUM NG: Target`, `fw-checksum.py`). Diese
Routine liegt im SRIC-Modul (`0x53d1dfd8`) und gehört zum **Update des
Sub-CPU** über das SRIC-Protokoll, nicht zur Prüfung der Update-Datei. Die
Container-Prüfsummen sind die beiden Wortsummen oben.

## Was offen bleibt

- **Prüfungen nach dem Schreiben.** Statisch findet sich keine Prüfsumme pro
  Partition und keine Prüfung der RTOS-Partition beim Booten: Der M0-Bootcode
  prüft nur die Werks-Justagedaten (`adjust check sum error!`). Bewiesen ist
  das nicht; die Kamera kann Dinge prüfen, die ohne Meldungstext im Code
  stehen.
- **Recovery.** Unverändert die Voraussetzung vor jedem Flash, siehe
  [firmware-update-security.md](firmware-update-security.md). Die gepackte
  Datei besteht die Prüfungen der Kamera — ob das geänderte Image danach
  bootet und richtig rechnet, zeigt erst die Hardware.
- **Geänderte Größe.** `fw-pack` unterstützt nur Änderungen ohne
  Größenänderung des Images; für Matrizen und Tabellen genügt das.
