# RAM-Loader: Looks über SD-Skripte statt Flash

Stand 2026-10-21. Plan: [../plan-ram-loader.md](../plan-ram-loader.md).
Bisher gebaut: Phase 0 (RAM-Dump und Vergleich), auf der Kamera gelaufen
(T1, siehe Ergebnisse), und Phase 1: der Editor exportiert einen Look als
RAM-Patch `LOOK.PAT` (`core/src/rampatch.rs`, Format dort). Phase 2, der
Loader, ist gebaut und am Mac getestet; auf der Kamera noch nicht gelaufen
(T2).

## Mechanismus

Übernommen als Idee von [grkit](https://github.com/ahonn/grkit) (MIT), das
auf GR III 2.10 so Filmlooks lädt:

1. **SD-Skripte einschalten** (einmalig, eine Kameraeinstellung): ModeSet-Datei
   `00078350.588` mit `[SCRIPT_EN_MODE EN]\r\n` und `DEVELOP.MOD` (10 feste
   Bytes) ins Wurzelverzeichnis, Kamera mit gedrückter MENU-Taste
   einschalten. Die Kamera zeigt „Mode Set Command → Complete“ und schaltet
   sich aus. Ausschalten genauso mit `[SCRIPT_EN_MODE DIS]\r\n`. Der
   Dateiname sind Major- und Minor-Nummer des RTOS (GR IIIx:
   `00078490.609`).
2. **`SCRIPT/STARTUP.TTL`** läuft bei jedem Einschalten im RTOS, eine
   Makrosprache ähnlich Tera Term (`sendln`, `filesearch`, `mpause`, `goto`).
   `sendln 'lcmd sh …'` führt ein Shell-Kommando im Linux der Kamera aus.
   Solange Linux die Karte nicht gemountet hat, schlägt `lcmd` fehl, daher
   wird wiederholt, bis eine Ergebnisdatei da ist. Pfade: `C:\…` im Skript ist
   `/media/storage1/…` in Linux. Kommentare (`;`) lassen wir weg, weil grkit
   keine benutzt und wir den Interpreter nicht kennen.
3. **Ein statisches ARM-Linux-Programm** ohne libc mappt `/dev/mem` und liest
   (Phase 0) bzw. schreibt (ab Phase 2) den RAM des RTOS.

## Adressen

Wie in `core/src/addr.rs`, bestätigt durch die Startup-Literale bei
`0x53000450` im Stock-Image:

| | Adresse |
|---|---|
| RTOS-Image im RAM | `0x53000000` = Dateioffset + `0x52fef790` |
| `.data`-Quelle im Image | `0x5407b540`, Größe `0x3a080` |
| `.data` zur Laufzeit | `0x55000000` |
| BSS | `0x5503a100..0x55804374` |
| Build-Kennung | Wort bei `0x53002038` = `0x1b150a02` |

## Phase 0: RAM-Dump

Dateien in `tools/ramload/`:

- `dump.c`: mappt `/dev/mem` nur lesend (`O_RDONLY | O_SYNC`, `PROT_READ`)
  und schreibt nach `SCRIPT/RAMLOAD/`:
  - `RTOS.BIN`: `0x53000000..0x540b6000` (Image-Ende aus den Literalen, auf
    4 KiB gerundet), 17,5 MB
  - `DATA.BIN`: `0x55000000..0x55805000` (`.data` und BSS), 8,4 MB
  - `INFO.TXT`: Literale, Bereiche, CRC-32 beider Dateien
  - `DUMP.OK` zuletzt, oder `DUMP.ERR` mit Fehlercode
  Prüft vor dem Lesen die Literale (`.data` muss bei `0x55000000` liegen),
  sonst `DUMP.ERR` mit Code 3/4.
- `card/dump.ttl` (wird `SCRIPT/STARTUP.TTL`), `card/DUMP.SH`: Start über
  `lcmd`, höchstens 2 Minuten Wiederholung.
- `build.sh`: baut `DUMP.BIN` und `LOAD.BIN` mit clang/lld
  (`--target=armv7-linux-gnueabi -mfpu=none -mfloat-abi=soft`; ohne
  `-mfpu=none` vektorisiert clang die CRC-Schleife mit NEON) und legt in
  `work/ramload/` an:
  - `SD_DUMP/` – Inhalt ins Wurzelverzeichnis der Karte: Dump
  - `SD_LOAD/` – Inhalt ins Wurzelverzeichnis, dazu `LOOK.PAT`: Loader
  - `SCRIPT_ON/`, `SCRIPT_OFF/` – ModeSet-Dateien zum Ein- und Ausschalten
  Es ersetzt nur diese vier Ordner. (Bis 2026-10-21 löschte es ganz
  `work/ramload/`; dabei gingen der Dump von T1 und die Firmware-Sicherung
  verloren, siehe Ergebnisse.)
- `test-dump.py`: führt `DUMP.BIN` in Unicorn gegen einen simulierten
  Kamera-RAM aus (Image bei `0x53000000`, `.data` kopiert, BSS null) und
  emuliert die Syscalls. `--poke ADDR=WORD` simuliert Schreibzugriffe der
  Firmware zur Laufzeit. Braucht `.venv/bin/python` (Unicorn).

`tools/ram-compare.py DUMPDIR [IMAGE …] [--builds]` vergleicht einen Dump mit
dem Stock-Image und beliebigen Builds (`FWDC239B.BIN` oder entpackt) und
beantwortet:

1. Welche Firmware läuft? Der Kandidat mit den wenigsten abweichenden Bytes
   in Code und Read-only-Daten (`0x53000000..0x5407b540`).
2. Ist der freie Bereich des Editors (`0x53000d50..0x53004000`) zur Laufzeit
   unverändert?
3. Haben die Export-Stellen zur Laufzeit noch ihre Werte aus der Datei?
   Export-Stellen sind alle Bytes, in denen ein übergebener Build vom Stock
   abweicht, plus die drei Look-Tabellen (`0x55018cf0`, `0x55018d94`,
   `0x55018e4c`).

Test in der Emulation (2026-10-21): Dump des `grain-test`-Builds mit zwei
simulierten Schreibzugriffen (`0x55018d94`, `0x53001000`). Der Vergleich
erkennt `grain-test` unter elf Kandidaten (4 Bytes Abweichung gegen 378 beim
Stock) und meldet genau die beiden Stellen. Der Dump des Stock-Images ergibt
0 Bytes Abweichung.

### Ablauf an der Kamera (T1)

1. `tools/ramload/build.sh`
2. SD-Skripte einschalten: Inhalt von `work/ramload/SCRIPT_ON/` ins
   Wurzelverzeichnis, Kamera aus, mit gedrückter MENU einschalten, warten bis
   sie sich selbst ausschaltet. Danach die beiden Dateien löschen.
3. Inhalt von `work/ramload/SD_DUMP/` ins Wurzelverzeichnis.
4. Kamera normal einschalten (ohne MENU), 2 Minuten warten, ausschalten.
5. `SCRIPT/RAMLOAD` von der Karte kopieren, dann:
   `python3 tools/ram-compare.py <ordner> --builds ~/Downloads/FWDC239B-*.BIN`
6. Ergebnis hier eintragen.

Wenn `DUMP.OK` fehlt: `DUMP.ERR` ansehen. Fehlen beide, hat `lcmd` nie
geklappt (SD-Skripte aus, oder Linux mountet die Karte anders).

**Wichtig:** Liegt ein Firmware-Update (`FWDC239B.BIN`) im
Wurzelverzeichnis, greift die ModeSet-Datei nicht. Die Datei vorher von der
Karte nehmen (T1: erst danach klappte es).

### Ergebnisse

**T1, 2026-10-21, GR III:** Der Dump lief beim ersten Einschalten nach dem
ModeSet durch (`DUMP.OK`). SD-Skript, `lcmd` und das Lesen von `/dev/mem`
funktionieren also. Die Startup-Literale und die Build-Kennung `0x1b150a02`
stimmen mit dem Stock-Image überein. Der Dump selbst ist verloren
(`build.sh` hat `work/ramload/` gelöscht); die Ergebnisse unten stammen aus
dem Vergleich davor. Die Firmware-Sicherung ist wiederhergestellt: aus
`my-look.looks.json` (noch auf der Karte) neu exportiert, SHA-256 gleich.

1. **Firmware:** Die Kamera läuft mit dem Build, der am 18.10. auf der Karte
   lag (`work/ramload/card-fw/FWDC239B.BIN`, SHA-256 `1d48bf8758c4ddca…`,
   Looks in `my-look.looks.json` daneben). Code und Read-only-Daten
   `0x53000000..0x5407b540` sind Byte für Byte gleich (0 Bytes Abweichung);
   der nächste Kandidat weicht um 11 643 Bytes ab, Stock um 13 102.
   Die einzige Abweichung im RTOS-Bereich liegt hinter dem Image-Ende
   (`0x540b55c0..0x540b6000`, Rundung auf 4 KiB).
2. **Freier Bereich** `0x53000d50..0x53004000`: zur Laufzeit unverändert.
3. **Export-Stellen** (138 Bereiche aus 21 Builds und den drei
   Look-Tabellen): alle unverändert. Die Firmware schreibt also weder die
   Look-Tabellen noch unsere Sets beim Start um.

In `.data` weichen sonst 1 375 Bytes in 50 Bereichen von der Datei ab,
Laufzeitvariablen.

Folge für Phase 1: Basis des RAM-Patches ist das Image, das die Kamera
gerade laufen hat, also derzeit der Build von der Karte, nicht Stock. Der
Patch muss vor dem Schreiben genau diese Bytes vorfinden.

## Phase 2: Loader

- `loadcore.h`: Prüfen und Schreiben, ohne Linux-Aufrufe. Vor dem ersten
  Schreibzugriff: `LOOK.PAT` unbeschädigt (Magic, CRC, Größen, jeder Bereich
  in Code oder `.data`), CRC-32 des Codes im RAM gleich der Basis des
  Patches, alte Bytes jedes Bereichs stimmen. Steht der Patch schon im RAM
  (zweiter Lauf nach demselben Einschalten), ändert er nichts. Danach in
  Dateireihenfolge schreiben, nach jedem Bereich `flush`; schlägt ein
  `flush` fehl, weicht das Zurücklesen ab oder stimmt der Code-CRC danach
  nicht, schreibt er alle Bereiche auf ihre alten Bytes zurück.
  Statuscodes stehen im Kopf der Datei.
- `load.c`: liest `SCRIPT/RAMLOAD/LOOK.PAT`, mappt `/dev/mem` lesend und
  schreibend (`O_RDWR | O_SYNC`) und schreibt `LOAD.OK` oder `LOAD.ERR`.
  `flush` ist `dsb sy` plus der ARM-Syscall `cacheflush`, wie bei grkit.
- `card/load.ttl`, `card/LOAD.SH`: wie beim Dump, höchstens 1 Minute.
- `test-load.sh [BASIS [PRESET]]`: baut `loadcore.h` am Mac mit einem
  Testprogramm (`test-load.c`) und spielt alle Fälle gegen Puffer mit dem
  RAM-Abbild der Basis durch (`view.py`): Laden, zweiter Lauf, andere
  Firmware (5), beschädigter Patch (4), Bereich außerhalb (6), alte Bytes
  falsch (7), teilweise geschrieben (8), `flush` scheitert (9), Schreiben
  geht verloren (10), Code-CRC danach falsch (11); nach jedem Fehler muss der
  RAM wie vorher sein. Gegenprobe: ohne Zurückschreiben schlagen 9–11 fehl.
  Nicht abgedeckt: die Linux-Aufrufe selbst (`open`, `mmap`, `cacheflush`).
- `core/examples/ram_patch.rs`: `LOOK.PAT` aus Preset und Basis, ohne
  Browser.

### T2: nur Daten

`work/ramload/t2/`: das Preset vom 18.10. unverändert, dazu Negative Film
als eigenes Ziel mit vertauschtem Rot und Blau (Matrix `[0,0,1, 0,1,0,
1,0,0]`). Der Patch gegen den Kamera-Build hat genau einen Bereich: 18
Bytes bei `0x53dc2ad0`, die 3×3-Look-Matrix von Negative Film (Q13, vorher
`[8939,-1642,895, 1247,5915,1030, 57,2165,5970]`). Keine Instruktion ändert
sich. Basis-CRC `1f906534`, danach `739d2e24`, SHA-256 von `LOOK.PAT`
`b5ebaba1…`.

Ablauf:

1. SD-Skripte sind seit T1 an. Kein `FWDC239B.BIN` im Wurzelverzeichnis.
2. Inhalt von `work/ramload/SD_LOAD/` ins Wurzelverzeichnis,
   `work/ramload/t2/LOOK.PAT` nach `SCRIPT/RAMLOAD/`.
3. Kamera normal einschalten, etwa 1 Minute warten.
4. Ein Foto mit Standard (Kontrolle), eines mit Negative Film: erwartet
   vertauschte Farben (blauer Himmel orange, Hauttöne bläulich), Standard
   unverändert. Auch das Live-Bild ansehen.
5. Ausschalten, Karte heraus, `SCRIPT` vom Mac aus umbenennen oder löschen,
   Karte zurück, einschalten: Negative Film wieder wie vorher.
6. `SCRIPT/RAMLOAD/LOAD.OK` bzw. `LOAD.ERR` und die Fotos auswerten.

## Wiederherstellung

Der Dump schreibt nichts in den Kameraspeicher; der Loader nur in den RAM,
bis zum Ausschalten. Hängt die Kamera trotzdem:
Akku raus, Karte raus, ohne Karte starten. Das `SCRIPT`-Verzeichnis löschen
deaktiviert das Skript; die SD-Skript-Einstellung bleibt an, bis sie mit
`SCRIPT_OFF` ausgeschaltet wird.
