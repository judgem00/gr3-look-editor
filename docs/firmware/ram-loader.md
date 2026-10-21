# RAM-Loader: Looks über SD-Skripte statt Flash

Stand 2026-10-21. Plan: [../plan-ram-loader.md](../plan-ram-loader.md).
Bisher gebaut: Phase 0 (RAM-Dump und Vergleich), auf der Kamera gelaufen
(T1, siehe Ergebnisse).

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
- `card/STARTUP.TTL`, `card/DUMP.SH`: Start über `lcmd`, höchstens 2 Minuten
  Wiederholung.
- `build.sh`: baut `DUMP.BIN` mit clang/lld
  (`--target=armv7-linux-gnueabi -mfpu=none -mfloat-abi=soft`; ohne
  `-mfpu=none` vektorisiert clang die CRC-Schleife mit NEON) und legt in
  `work/ramload/` an:
  - `SD_ROOT/` – Inhalt ins Wurzelverzeichnis der Karte
  - `SCRIPT_ON/`, `SCRIPT_OFF/` – ModeSet-Dateien zum Ein- und Ausschalten
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
3. Inhalt von `work/ramload/SD_ROOT/` ins Wurzelverzeichnis.
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
stimmen mit dem Stock-Image überein. Dump in
`work/ramload/dump-2026-10-21/`.

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

## Wiederherstellung

Der Dump schreibt nichts in den Kameraspeicher. Hängt die Kamera trotzdem:
Akku raus, Karte raus, ohne Karte starten. Das `SCRIPT`-Verzeichnis löschen
deaktiviert das Skript; die SD-Skript-Einstellung bleibt an, bis sie mit
`SCRIPT_OFF` ausgeschaltet wird.
