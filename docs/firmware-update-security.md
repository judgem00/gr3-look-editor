# Update-Sicherheit der GR III — Analyse

Ergebnis von Phase 1 des Farbprofil-Plans: Prüft die
Kamera modifizierte Firmware kryptografisch, oder nur auf Integrität?

**Kurz: keine Kryptografie. Nur eine nachrechenbare Prüfsumme.**

## Vorgehen

Firmware `fwdc239b`, entpackt nach `assets/fwdc239b/unpackedFW`. Ladeadresse
des RTOS/DSP-Blobs bekannt (Bias `0x52FEF790`, siehe
`tools/find-load-address.py`), damit sind
String-Referenzen im Code auflösbar.

## Was nach Signatur aussah — und was es wirklich ist

| Fund im Image | Realität |
|---|---|
| `No valid EB signature found` | **FAT-Dateisystem** — der `0xAA55`-Bootsektor-Marker im SD-Karten-Formatierer |
| `%sPRSA -------,0x%08x` | **Hardware-Register** `ProReadUnit` (PR + Source Address), Bildpipeline |
| `RSA}`, `{AeS`, `#MD5` | Zufalls-Bytes mit falschem 4-Byte-Alignment, keine Symbolnamen |
| `CHECKSUM NG`, `checksum error!!` | eine **additive Prüfsumme**, kein Signaturvergleich |

Im gesamten Image findet sich **keine Kryptobibliothek** — kein RSA, SHA, AES,
HMAC, kein mbedTLS/OpenSSL/wolfSSL. Ein signaturgeprüfter Updater braucht die
zwingend.

## Der Update-Pfad

```
UserFwUpdateController / FactoryFwUpdateController
   -> src/FwUpdate.cpp
        -> CpuFwUpdate / DspFwUpdate / BothFwUpdate
             -> SRIC-Protokoll -> Ziel-Bootloader (SAMBA-Z)
                  BOOT DOWNLOAD -> BOOT VERIFY -> CHECKSUM
```

Fehlermeldungen des Updaters (vollständig): `Low Battery`, `File Bloken`,
`Update Failure`, `Unknown`. **Keine** Signatur-/Vertrauens-Fehlermeldung.

Intern lädt die Kamera die Datei als `C:\FWDC239B.BIN` und prüft den
`RICOH`-Header (Datei beginnt mit `RICOH\0\0\0GR III`).

## Die Prüfung im Maschinencode

Routine bei RAM `0x535796c0` / `0x53579790` (Datei-Offset ~5.808.000), die
`CHECKSUM NG: Target:%02d` protokolliert:

```
53579708  ldrb r1,[r0]; add r2,r2,r1,lsl#24    ; Byte, Index % 4 == 0
53579774  ldrb r1,[r0]; add r2,r2,r1,lsl#16    ; Index % 4 == 1
53579730  ...          add r2,r2,r1,lsl#8      ; Index % 4 == 2
53579744  ...          add r2,r2,r1            ; Index % 4 == 3
...
537597c4  ldrb r3,[r5,#15]   ; gespeichertes Prüfbyte aus dem Header
537597c8  ldrb r2,[fp,#-36]  ; gerade berechnetes Byte
537597cc  cmp  r2,r3
537597d0  beq  ok            ; sonst -> CHECKSUM NG
```

Die Bytes werden in ein 32-Bit-Register akkumuliert (äquivalent: Summe der
Daten als Big-Endian-32-Bit-Wörter), auf ein Byte gefaltet und gegen **ein
einzelnes gespeichertes Byte** verglichen. Nichts, was eine modifizierte Datei
nicht neu berechnen könnte. Der Algorithmus ist in
`tools/fw-checksum.py` reproduziert.

## Bedeutung

Der Update-Pfad wehrt sich technisch nicht gegen modifizierte Firmware. Damit
ist **Gate 1 des Plans vorsichtig positiv**: echte eigene Farbprofile sind
grundsätzlich flashbar.

## Offene Vorbehalte

1. **Statische Analyse, keine Ausführung.** Der Befund beruht auf gelesenem
   Code und fehlenden Krypto-Strings — starke Evidenz, aber der endgültige
   Beweis wäre, den Updater im Emulator gegen eine modifizierte Datei laufen zu
   lassen.
2. **Recovery-Pfad ungeklärt.** `SAMBA-Z`/`SRIC` deutet auf einen
   Bootloader-Modus, der Wiederherstellung erlauben könnte — muss bestätigt
   werden, bevor irgendjemand flasht. Ohne Recovery bleibt Bricking real.
3. **Packen der `.bin` — gelöst.** Siehe [firmware-packing.md](firmware-packing.md).
   Korrektur: Die oben beschriebene Byte-Summe gehört zum SRIC-Update des
   Sub-CPU; die Update-Datei selbst ist durch zwei Wortsummen gesichert.

---

# Recovery-Pfad — Analyse

Zweiter Teil von Phase 1: Gibt es einen Rückweg, wenn ein Flash fehlschlägt?

**Kurz: Ja, mehrere Anzeichen für Recovery — aber keiner am echten Gerät
bestätigt. Vor einem Flash zwingend praktisch verifizieren.**

## Die Boot-Architektur

Vier Segmente im Image, die auf getrennte eMMC-Partitionen abgebildet werden
(Adressen aus dem `PARTPARA`-Header):

| Segment | Datei-Offset | Rolle |
|---|---:|---|
| RTOS/DSP/ISP + Tuning | 11,2–16,3 MB | Bildpipeline, **hier liegen die Farbtabellen** |
| Bootloader `stubby-sc2000-rtos` | 16,8 MB | lädt Kernel + DTB aus dem romfs |
| romfs (Kernel, `sc2000.dtb`) | 16,8 MB | |
| cpio (Linux-Rootfs) | 19,7 MB | |
| **M0-Supervisor** (`KB588CPU project`) | 33,0 MB | eigener Always-On-Kern |

Der **M0-Supervisor** ist der Schlüssel: ein kleiner, immer laufender Kern für
Laden, Charging, Power und DSP-Reset — in einer **eigenen Partition**, getrennt
von der Hauptfirmware.

## Was das Update tut (Brick-Risiko)

`src/CpuFwUpdate.cpp` / `DspFwUpdate.cpp` schreiben **direkt in den eMMC**:

```
make partition table -> write partition table
write boot header
write boot image (M0 Code)
write partition data
format resource drive -> mount resource drive
```

Das ist ein **In-Place-Flash**. Eine Unterbrechung mitten im Schreiben (Akku
leer, Abschaltung) ist der Brick-Fall. Deshalb prüft der Updater vorab den
Akku (`Low Battery`) und die Projektnummer (`project number is different!` —
eine modifizierte Firmware muss die Projektnummer erhalten).

## Anzeichen für Recovery

**1. SD_BOOT-Modus (Hardware-nah).** Der M0-Supervisor kennt einen SD-Boot-
Modus und wechselt bei fehlgeschlagenem Handshake mit dem Hauptsystem hinein:

```
Selected SD_BOOT MODE.
HS error, SD BOOT mode.
```

Ausgelöst wird er über Tastenzustände beim Einschalten (`SWMAIN`,
`SWPLAY_LONG` u. a.) — das klassische Ricoh/Pentax-Recovery-Muster (Taste
halten + Firmware auf SD-Karte). Da der M0 in einer eigenen Partition liegt,
überlebt er einen Brick der Hauptfirmware.

**2. Service-Modus mit Dump/Restore.** `src/FlashMemoryFactory.cpp` bietet
einen Werkzeugsatz auf Flash-Ebene:

```
dump drive     -> C:\dump_%s.bin / C:\fwdump.bin   (Flash sichern)
restore drive  <- load dump file                   (Flash zurückschreiben)
format drive / copy drive / break drive
```

Damit lässt sich der Flash vor einem Experiment vollständig sichern und danach
zurückschreiben — ein eingebautes Backup.

**3. Factory-Full-Update.** Neben dem User-Pfad gibt es
`FactoryFwUpdateController` mit `DSP & CPU FULL Update FW` — ein vollständiger
Reflash, der mehr überschreibt als das normale Update.

## Ehrliche Bewertung

Ein Recovery-Mechanismus **existiert nachweisbar** im Code — SD-Boot des
M0-Supervisors, Service-Dump/Restore, Factory-Full-Update. Das senkt das
Brick-Risiko deutlich: Der M0 liegt separat und hat einen SD-Boot-Pfad.

**Was statisch nicht beweisbar ist:**

- Ob SD_BOOT ein vollständig gebricktes Hauptsystem wiederherstellt (die ganze
  Firmware von SD lädt) oder nur die begrenzte M0-Funktion
- Der genaue Tastentrigger für den Recovery-Modus
- Ob der Maskrom des SoC (die unflashbare erste Stufe) einen USB-Download-Modus
  hat (die `SAMBA-Z`-Strings deuten darauf hin, sind aber nicht eindeutig)

**Konsequenz für das Vorgehen:** Nicht blind flashen. Vor dem ersten Flash
- den genauen SD_BOOT-Trigger in der GR-Service-Szene recherchieren,
- mit dem Service-Dump ein vollständiges Flash-Backup ziehen,
- und den Recovery-Weg an der Hardware verifizieren, **bevor** etwas
  Modifiziertes geschrieben wird.

Erst wenn der Rückweg praktisch bestätigt ist, ist ein Flash-Experiment
vertretbar.
