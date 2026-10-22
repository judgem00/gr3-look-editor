# ND im Video-M: Monitor statt Flash (Stand 2026-10-22)

Neuer Anlauf für das ruhende Thema „ND-Filter im Video-M“
([video.md](video.md), Abschnitt „ND-Filter im Video-M“), mit den
Werkzeugen aus [ram-loader.md](ram-loader.md) (Idee von grkit):

- **Video-M ohne Flash:** die Video-Patches (`manual mlive7 dials osd
  bitrate`) als RAM-Patch, 9 Code-Stellen.
- **Zuschauen statt raten:** ein Programm, das während der Bedienung
  die Felder mitliest, an denen der ND-Antrieb hängt. Es mappt `/dev/mem`
  nur lesend, wie der Dump.

Die drei Patches von 2026-10-16 (`nd`, `ndprobe`, `ndaction`) haben je
einen Flash gekostet und nur „fährt nicht“ ergeben. Offen blieb, *wo* die
Kette in Video-M reißt. Das soll ein einziger Kameralauf jetzt zeigen.

## Neue statische Befunde

### Der Liveview-Weg des ND

Der ND fährt im Liveview (Foto und Video) über die Schleife des
**LiveViewProcess** (`src/LiveViewProcess.cpp`, vtable `0x53c1d000`, im
Folgenden `lvp`), nicht über den Einstellungs-Pfad:

1. `0x534448f8(lvp, …, ae)` entscheidet, ob eine ND-Änderung ansteht, und
   setzt dann `[lvp+0x9f0d] = 1`. Sperren: `[lvp+0x760] != 0`,
   `[lvp+0x728] & 0xb`; danach ruft es die ND-Zustandsmaschine
   `0x5344d090(ae, Bit 8 von [lvp+0x728])`.
2. In der Ereignisschleife `0x534485bc` (`0x53448af8`): steht
   `[lvp+0x9f0d]`, geht `0x534449d4` mit „rein?“ = `[ae+0x56a0]` und
   Dichte = `[ae+0x569c]` an den einzigen FSS-Absender `0x53454a9c`
   (Nachricht `0x64c40802`).
3. Der FSS-Handler `0x5345d514` ruft den Treiber ND-CLOSE `0x536f15dc` /
   ND-OPEN `0x536f143c` mit dem ND-Objekt aus `[0x5508550c]`.

`ae` ist der AE-Kontext `[0x55085104+0x10]` (Singleton
`LiveViewServiceAeAfAwb`, Getter `0x5345558c`; `lvp+0xaa0` zeigt auf
denselben).

### Die ND-Zustandsmaschine `0x5344d090`

Sie arbeitet nur, wenn das Zustandsbyte `[ae+0x555d]` den Wert 3, 7 oder 4
hat (`& 0xfb == 3` bzw. `== 4`); Anforderungen stehen als Bits in
`[ae+0x5694]` (0x20, 0x40, 0x1/0x4, 0x2/0x8, 0x80, 0x100), dazu die
Globals `0x550850c1`, `0x550850c3`, `0x550850fc`. Bei jedem anderen Wert
von `[ae+0x555d]` setzt sie kein Ergebnis — dann bleibt `[lvp+0x9f0d]` 0
und der Filter fährt nicht. Das passt zum Befund „Symbol ja, Antrieb
nein“: das Symbol kommt aus der Einstellung, der Antrieb aus dieser
Maschine.

**Verdacht** (am Gerät zu prüfen): `mlive7` schreibt im Video-M jedes Bild
den Modus 5 (M) in das AE-Feld `+0x8e`; der Video-AE in M bringt
`[ae+0x555d]` oder die Bits in `[ae+0x5694]` nicht in den Zustand, den die
Maschine braucht.

### Der Treiber selbst

- ND-CLOSE `0x536f15dc(obj, force)`: ohne `force` nichts, wenn
  `[obj+0x38]` schon 1 ist. Fährt über drei Schreibbefehle an den
  Motortreiber (`0x5373237c` → SIO-Befehl an den BU24026), setzt
  `[obj+0x38] = 1` und **zählt jede Fahrt** in
  `[[[obj+0x28]+4]+0x54]` (`0x536f16c0`). Den Zähler liest der Monitor.
- Das ND-Objekt ist statisch: `0x5536dc58` (Getter `0x536f0df0`).
- Die Debug-Funktionen „ND-OPEN.“/„ND-CLOSE.“ (`0x53046dd4`/`0x53046e1c`,
  `src/MotorCommand.cpp`) haben die Form eines Shell-Kommandos
  `(argc, argv)`, sind aber nirgends registriert (kein Zeiger, kein
  Aufruf im Image). Per `sendln` aus dem SD-Skript sind sie also nicht
  erreichbar.

### Speicherschutz

Die MMU-Tabelle des RTOS (`TTBR0 = 0x53004000`, `DACR = 0x55555555`)
markiert `0x51600000..` (Code, Read-only-Daten) als **nur lesbar**
(AP=2, APX=1), `0x55000000..` (`.data`, BSS, Heap) als lesbar und
schreibbar. Folge für Code-Patches: Zähler oder Variablen in der freien
Code-Fläche `0x53000d50..` führen zu einem Data Abort. Linux schreibt dort
über `/dev/mem` (T2/T3), das RTOS selbst nicht. Der Heap reicht bis
`0x57004380` (`0x53852338`), davor ein 64-Byte-`sbrk`-Pool
`0x55804380..0x558043c0`; hinter dem BSS-Ende ist also kein freier Platz.

## Der Monitor

`tools/ramload/ndmon.c` → `NDMON.BIN`, gestartet von `NDMON.SH` im
Hintergrund, läuft 3 Minuten. Prüft zuerst Build-Kennung `0x1b150a02` und
BSS-Ende, sonst `NDMON.ERR`. Mappt `0x55000000..0x5a000000` nur lesend,
sucht `lvp` auf dem Heap (vtable und `+0xaa0 == ae`; ein zweites Objekt
mit derselben vtable wird so verworfen) und fragt alle 20 ms ab:

| Name | Feld |
|---|---|
| `nd_state`, `nd_moves` | `[0x5536dc58+0x38]`, Fahrtenzähler |
| `nd_ptr` | `[0x5508550c]` (ND-Objekt des FSS-Handlers) |
| `g_550850c0`, `g_550850fc` | Globals der Zustandsmaschine |
| `ae+5558` … `ae+56a8`, `ae+b6e4` | Zustandsbyte `+0x555d`, Bits `+0x5694`, Dichte `+0x569c`, „rein?“ `+0x56a0` |
| `lvp+728` … `lvp+ac4` | Freigabe `+0x728`, Zustand `+0x730`, Sperre `+0x760`, ansteht `+0x9f0d` |
| `a94+24f8`, `a94+2518` | Sperren aus `[lvp+0xa94]` (`0x534466d4`, `0x53444948`) |

Ausgabe in `SCRIPT/RAMLOAD/`:

- `NDMON.LOG`: Startzeile (Linux-Uhrzeit), jede Änderung mit Zeit in ms,
  alle Werte alle 10 s.
- `SNAP.BIN`: Datensätze `{"SNAP", ms, Adresse, Länge}` + Bytes. ND-Bereich
  des AE-Kontexts (`+0x5000..+0x5800`, `+0xb600..+0xb800`), `lvp`
  (`0xb000` Bytes), `[lvp+0xa94]+0x2400`, ND-Objekt: beim Start, alle 10 s
  und nach jeder Änderung (höchstens 1/s). `.data` und BSS (8,4 MB) alle
  30 s. Insgesamt etwa 140 MB. Damit lassen sich auch Felder finden, die
  nicht in der Liste stehen (Vergleich Video-P gegen Video-M).
- `NDMON.OK` am Ende (`lvp`, Bytes), oder `NDMON.ERR`.

Test am Mac: `tools/ramload/test-ndmon.py OUTDIR [IMAGE]` führt `NDMON.BIN`
in Unicorn gegen simulierten RAM aus (Heap bis `0x57005000`, ein Köder-
und ein echtes `lvp`, simulierte Zeit, ein ND-Druck bei 5 s und ein
AE-Bit bei 20 s). Bestanden 2026-10-22: `lvp` gefunden, genau die fünf
Änderungen geloggt, kein Schreibzugriff auf den gemappten Speicher, 261
Snapshots, 137 MB.

## Kameralauf ND1

Karte: `work/ramload/nd1/CARD/` (aus `tools/ramload/build.sh`, Ordner
`SD_NDMON`, dazu `nd1/LOOK.PAT`). `STARTUP.TTL` lädt erst den Patch
(`LOAD.OK`), dann startet es den Monitor (`NDMON.LCK`).

`LOOK.PAT`: nur die Video-Patches (`{"targets": {}, "video": true}`) gegen
**Stock** 2.10 (Basis-CRC `2773baf7`, danach `144b58eb` — derselbe Code wie
der geflashte und getestete Build `video-test-osd`), 9 Bereiche, 117 Bytes,
SHA-256 `82fbfde5…`. Erster Entwurf (gegen den Build vom 18.10.) siehe
Lauf 1.

Ablauf:

1. SD-Skripte sind seit T1 an. Kein `FWDC239B.BIN` im Wurzelverzeichnis.
   Die Kamera läuft mit Stock 2.10.
2. Inhalt von `work/ramload/nd1/CARD/` ins Wurzelverzeichnis (ersetzt
   `SCRIPT/STARTUP.TTL`).
3. Kamera im **Foto-Modus P** einschalten, ND aus. 1 Minute warten (Patch
   und Monitor starten). Uhrzeit notieren.
4. Jede Phase mit einem Markierungsfoto beginnen (Dateizeit = Zeitmarke),
   dann ND-Taste: an, 5 s warten, aus, 5 s warten.
   - A: Foto P
   - B: Video P (Markierung: 2 s Video)
   - C: Video M — erst prüfen, dass M wählbar ist (dann wirkt der Patch)
   - D: Video M noch einmal, diesmal ND an lassen und 10 s warten
   - E: Foto M (Kontrolle: fährt der ND in Foto-M ab Werk?)
5. Hören: rastet der Filter ein? Pro Phase notieren.
6. Kamera bis zum Ende der 3 Minuten anlassen (oder ausschalten; das Log
   ist bis auf die letzten 2 s gesichert).
7. `SCRIPT/RAMLOAD/` (Log, `SNAP.BIN`, `LOAD.*`, `NDMON.*`) und die
   Markierungsdateien auf den Mac kopieren.
8. Zurück: `SCRIPT` löschen, Kamera aus und ein — alles wie vorher.

Erwartung, die die Daten entscheiden:

- Video P: `lvp+9f0c` (Byte `+0x9f0d`) kurz 1, `nd_moves` +1,
  `nd_state` 0→1.
- Video M, Kette reißt **vor** der Maschine: `ae+555c` (Byte `+0x555d`)
  nicht 3/4/7 oder keine Bits in `ae+5694` → Patch-Ort ist die Maschine
  bzw. ihr Eingang.
- Video M, `9f0d` steht, aber `nd_moves` bleibt: Sperre zwischen
  Ereignisschleife und FSS (`[lvp+0x9f0d]`-Zweig `0x53448af8`).
- `nd_moves` steigt, Filter fährt nicht: Hardware-/Treiber-Ebene.

Der nächste Patch wird dann als RAM-Patch getestet, nicht geflasht.

### Lauf 1 (2026-10-22, 17:28): gescheitert, zwei Ursachen

- `LOAD.ERR` 5, Code-CRC im RAM `2773baf7`: das ist **Stock 2.10**, nicht
  der Build vom 18.10. (`1f906534`), gegen den der Patch gebaut war. Die
  Kamera wurde zwischen T3 und Lauf 1 auf Stock zurückgeflasht. Kein
  Video-M; der Loader hat nichts geschrieben. Neuer `LOOK.PAT` gegen Stock.
- Der Monitor startete (`NDMON.LCK`, `NDMON.LOG` und `SNAP.BIN` angelegt),
  hinterließ aber weder Zeile noch `NDMON.ERR`; das Log wurde etwa 12 s nach
  dem Start geschlossen. Er starb also vor dem ersten Schreiben, während der
  Suche nach `lvp` oder der ersten Abfrage: entweder von außen beendet (mit
  der Shell von `lcmd`) oder ein Lesefehler auf `/dev/mem`. Änderungen:
  - löst sich per `fork` + `setsid` von der Shell; SIGHUP, SIGINT, SIGPIPE,
    SIGTERM werden geloggt und ignoriert,
  - SIGBUS, SIGSEGV, SIGILL schreiben `NDMON.ERR` (Fehler `0x100 + Signal`,
    Detail = Fehleradresse) und eine Zeile ins Log,
  - Fortschrittszeilen, sofort gesichert: `ae`, `nd_state`, `scan` alle
    4 MB der Heap-Suche, `scan done`, `first snapshot done`.
  Die Unicorn-Prüfung ist wieder bestanden; die Signal-Wege selbst kann
  Unicorn nicht auslösen. Dateien von Lauf 1: `work/ramload/nd1/run1/`.

### Lauf 2 (2026-10-22, 17:33): Video-M aus dem RAM, Monitor halb blind

- `LOAD.OK` (9 Bereiche, Basis `2773baf7`, danach `144b58eb`): **Video-M
  wirkt als RAM-Patch, ohne Flash.** Im Video-M kommt das ND-Symbol, der
  Filter rastet nicht ein (Beobachtung Nutzer).
- `NDMON.LOG` leer, `SNAP.BIN` endet bei 81 s (letzte Änderung 17:35:10),
  danach nahm die Kamera zwei Videos auf (17:35:14, 17:35:22). Auch in
  Lauf 1 kamen Fotos (17:28:36, :52), bevor Log und Snapshots Daten
  hatten. **Verdacht: Schreibt das RTOS auf die Karte (Foto/Video), überschreibt
  es Verzeichnis und FAT, die Linux gleichzeitig ändert.** Folge: während
  der Monitor läuft, weder fotografieren noch filmen.
- Das AE-Objekt liegt nicht im Heap des Startcodes: `[0x55085104+0x10] =
  0x597cdcf0`; die meisten Heap-Zeiger aus `.data` zeigen auf
  `0x59600000..0x59d00000`. Der Monitor las bis `0x57005000` und fand
  deshalb weder `ae` noch `lvp`. Jetzt: Map bis `0x5a000000`, Suche
  `0x59000000..0x5a000000`, Laufzeit 3 Minuten.
- Gesichert sind 15 Snapshots des ND-Objekts. `[0x5536dc58+0x38]`
  (Filter rein laut Treiber):

  | ms | Zustand | sonst geändert |
  |---|---|---|
  | 58 578 | 0 → 1 | |
  | 67 576 | 1 → 0 | |
  | 73 281 | 0 → 1 | `+0x2d` 4 → 0, `+0x30` 0 → `0x1361f` |
  | 74 380 | 1 → 0 | `+0x30` → `0x149da` |
  | 77 627 | 0 → 1 | |
  | 81 196 | 1 → 0 | |

  Der Treiber wurde also drei Mal zum Einfahren aufgerufen (das Feld setzt
  nur ND-CLOSE, `0x536f16a4`). In welchem Modus jeweils, ist ohne Log offen.
  `[0x5508550c]` = `0x5536dc58` (FSS-Handler und Shell nutzen dasselbe
  Objekt). Dateien: `work/ramload/nd1/run2/`.

### Lauf 3 (2026-10-22): Video M, ND drei Mal an/aus — die Kette reißt vor der Maschine

Monitor vollständig (`NDMON.OK`, `lvp = 0x599bd6d0`, `ae = 0x597e2b50`),
nur Video M, keine Aufnahme. Dateien: `work/ramload/nd1/run3/`.

- Beim Wechsel in den Videomodus (≈ 10 s) fährt der Treiber drei Mal
  (`nd_state` 1→0→1→0, `lvp+9f0d` gesetzt, `ae+56a0` folgt): der Antrieb
  funktioniert im Video, der Weg Zustandsmaschine → `9f0d` → FSS → Treiber
  auch.
- Danach, während der ND-Drücke in Video M, **nichts davon**:
  `ae+5694` (Anforderungs-Bits) die ganzen 3 Minuten 0, `[ae+0x555c..d]`
  = 0, `lvp+9f0d` nie gesetzt, kein Treiberaufruf. Die ND-Taste erreicht
  den AE-Kontext in Video M überhaupt nicht.
- Was sich zu den Drücken ändert (Kandidaten): bei 69,96 s
  `lvp+728` |= 0x10 für 35 ms (die „Freigabe“ aus `0x534466c4`) und
  `lvp+ac0` Byte 0; bei 90,0–96,9 s `lvp+760` = 2 (sonst 0) und
  `lvp+9f0a`, `a94+24fb`; bei 108,6 und 130,6 s das Global `0x550850c0`
  kurz 0.
- Per Bild ändern sich `lvp+754`, `lvp+768`, `lvp+ac4`, `a94+2518`
  (Rauschen).

Folge: Es fehlt der Vergleich mit Video P (wo die Taste wirkt). Der
Monitor vergleicht deshalb jetzt zusätzlich ganze Bereiche Wort für Wort
(AE-Kontext `+0x5000..+0x5800`, `+0xb600..+0xb800`, `lvp` komplett,
ND-Objekt jede Abfrage; `.data`/BSS in 64 Scheiben, ein Durchgang ≈ 1,3 s)
und schreibt Zeilen `d ADRESSE alt -> neu`; ein Wort, das sich mehr als 8
Mal ändert, wird mit `d!` abgeschlossen und dann ausgelassen. `.data`
wird nur noch am Anfang und Ende ganz gesichert.

Lauf 4: Foto einschalten, 1 Minute warten; Video P: ND an, 5 s, aus,
zwei Mal; 20 s warten; Video M: ND an, 5 s, aus, zwei Mal; nichts
aufnehmen, bis 3 Minuten um sind.

### Lauf 4 (2026-10-22): Video P, dann Video M — Abfrage zu langsam

`NDMON.OK`, aber nur 36 Abfragen in 3 Minuten (alle ~18 s). Ursache: ein
Text-Log des RTOS im BSS (`0x553cf74c..0x55543474`, ASCII-Einträge, keine
Code-Referenz dazwischen; letzte Variable davor `0x553bea34`) füllt sich
laufend, jedes Wort ändert sich einmal, also griff die Rauschgrenze nicht:
215 000 Diff-Zeilen, das Schreiben auf die Karte bremste die Schleife. Die
ND-Drücke (je 5 s) fielen zwischen die Abfragen. Dateien:
`work/ramload/nd1/run4/`.

Änderung: `0x553c0000..0x55560000` ausgeschlossen, höchstens 128
Diff-Zeilen pro Abfrage (Rest als `d+ N more`). Lauf 5 = Protokoll von
Lauf 4.

### Lauf 5 (2026-10-22): Video P gegen Video M — die Ursache

Abfrage alle 20–60 ms, größte Lücke 1,4 s. Dateien: `work/ramload/nd1/run5/`.

Video P, ND an (53,2 s, 62,2 s) und aus (57,7 s, 67,2 s), jedes Mal gleich:

1. `[ae+0x5558]` := neue Stellung, `[ae+0x555a]` := 1 („anwenden“),
   `[ae+0x5691]` := neue Stellung.
2. 30 ms später die Zustandsmaschine (`0x5344d240`): `[ae+0x5684]` := 1,
   `[ae+0x56a0]` := `[ae+0x5691]`, `lvp+9f0d` := 1.
3. Nach weiteren 30 ms fährt der Treiber (`nd_state`).

Video M (ab ≈ 99 s): Schritt 1 kommt nie.

Schritt 1 schreibt `0x53454288` (AE-Aktualisierung, aufgerufen von
`0x53441fa8` im Liveview-Ablauf): sie holt die Einstellungen aus dem
Einstellungsobjekt `[ae+0x5544]` (Getter `0x5356c6xx`, ND = Byte `+0x38`,
`0x5356c688`), schreibt die ND-Stellung nach `+0x5558`/`+0x5691` und setzt
bei einer Änderung `+0x555a`/`+0x5559` (`0x534544a4`; Sperren `+0x555b`,
`+0x5694 & 5`, `+0x5684`; `0x53666a88` liefert immer 0). **Steht aber
`[ae+0x428b]` auf 1, holt sie keine Einstellungen**, sondern behält die
alten Werte (`0x534542e4`) — und damit auch die ND-Stellung. `+0x428b`
ist der Merker „feste Belichtung“: gesetzt von `0x5343fbec` zusammen mit
den festen Werten (`ae+0x4294..0x42ac`) und von den Nachrichten
`0x68c40807` (1) / `0x68c40808` (0) über `0x53450a40`. Im Video M (mlive7)
ist er gesetzt; die ND-Taste ändert nur die Einstellung (Symbol), der AE
übernimmt sie nie.

### Patch `ndm`

Gruppe `ndm` in `core/src/video.rs` (nicht im Standardpaket `MANUAL`):

| Stelle | Änderung |
|---|---|
| `0x5301a78c` | Stub, 92 Bytes, im toten `AE_Mode`-Handler hinter dem `osd`-Stub (frei bis `0x5301a878`) |
| `0x5345431c` | Zweig „feste Belichtung“ von `0x53454288`: `strb r1,[r4,#0x62c]` → `bl` Stub |

Der Stub führt die ersetzte Instruktion aus und dann nur den ND-Teil des
anderen Zweigs: Sperren `+0x555b`, `+0x5694 & 5`; Stellung aus
`[[ae+0x5544]+0x38]` nach `+0x5558`/`+0x5691`; weicht sie von `+0x5690`
ab und ist `+0x5684` frei, `+0x555a`/`+0x5559` := 1. Belichtungswerte
fasst er nicht an; die Dichte (`+0x568c`) bleibt die zuletzt gesetzte.
Nebenwirkung: auch in Foto-M (falls dort `+0x428b` gesetzt ist) übernimmt
der AE die ND-Stellung jetzt im Liveview — das tut er in P ab Werk.

Geprüft: Stub mit Capstone und in Unicorn (Patchstelle → Stub → zurück,
r4–r8 und sp erhalten; 7 Fälle: an, aus, unverändert, Fahrt läuft noch,
Sperre `555b`, Sperre `694`, kein Einstellungsobjekt — alle wie der
P-Zweig). `LOOK.PAT` gegen Stock mit `manual mlive7 dials osd bitrate ndm`:
10 Bereiche, 213 Bytes, Basis `2773baf7`, danach `352f085f`, SHA-256
`378fd0ae…`. Der Bereich mit allen Stubs (`0x5301a730`, 184 Bytes) liegt
vor allen Sprungstellen und wird daher zuerst geschrieben. Karte:
`work/ramload/nd1/CARD-ndm/` (mit Monitor).

Lauf 6: wie Lauf 5 (Video P, dann Video M, je zwei Mal ND an/aus).
Erwartet in M: hörbares Einrasten, im Log `ae+5558`/`555a`, `lvp+9f0d`,
`nd_state` wie in P.

### Lauf 6 (2026-10-22): `ndm` v1 — in M ohne Wirkung

`LOAD.OK` (10 Bereiche, danach `352f085f`). Video P: vier Fahrten wie ab
Werk (21,6 / 26,1 / 30,5 / 34,9 s). Video M (ab ≈ 70 s): keine Fahrt, kein
`+0x555a`; Nutzer: rastet nicht ein, Belichtung unverändert.

Grund: Das Einstellungsobjekt `[ae+0x5544]` (= `ae+0x53cc`, liegt also
in den AE-Snapshots) bekommt die ND-Stellung `+0x38` in Video P bei jedem
Druck (22,3 / 26,4 / 30,6 / 34,7 s), **in Video M nie**. Geschrieben wird
`+0x38` nur vom Setter `0x5356c5e8`, und der wird im Pro-Bild-AE
`0x53472e30` (`0x534731b4`) aus der Belichtungsausgabe `[this+0x1474]+
0x90..0x97` versorgt — im M-Fall gefüllt aus der festen
Liveview-Belichtung (`0x5346f850`), die ihre ND-Stellung nur über den
AE-Befehl `SetLiveViewNdFilter` (`af9=0x1a`) bekäme. Der v1-Stub lief also
vermutlich, las aber eine Einstellung, die sich in M nicht ändert.

### `ndm` v2

Der Stub nimmt im Video (`[ud+0x44] == 1`) die Stellung direkt aus der
App-Einstellung **MovieNdFilter** (`[ud+0x530]`, 1 = an, 2 = aus; UserData
= `[0x55063f34]`, Modell `0x55063f30`, Getter `0x5320b31c`; Movie-Setter
`0x53253784`), sonst wie v1 aus `[[ae+0x5544]+0x38]`. 140 Bytes ab
`0x5301a78c` (Platz bis `0x5301a878`). Unicorn: 10 Fälle bestanden (Video
an/aus/schon so, Foto über Einstellungsobjekt, kein UserData, Sperren).
`LOOK.PAT`: 10 Bereiche, 261 Bytes, danach `645c1359`, SHA-256
`414fce00…`.

Monitor: Map bis `0x5e000000`, neue Felder `ud+44`, `ud+172`, `ud+530`.

Lauf 7 = Protokoll von Lauf 5.

### Lauf 7 (2026-10-22): `ndm` v2 — ohne Wirkung, Monitor stumm

`LOAD.OK` (danach `645c1359`). Nutzer: in Video M wieder kein Einrasten.
`NDMON.LOG` und `SNAP.BIN` leer, kein `NDMON.ERR`: der Monitor starb vor
der ersten Zeile, nach dem Anlegen des Logs. Einzige Änderung gegenüber
Lauf 6 war die Map bis `0x5e000000` (144 MB). Jetzt wieder bis
`0x5a000000`; die UserData-Seite (`[0x55063f34]`, in Lauf 6
`0x5d520c80`) wird einmal beim Start als eigene 8-KB-Map dazugenommen.

### `ndm` v3: beide Zweige

Unklar ist, ob Video M überhaupt durch den Zweig „feste Belichtung“
(`[ae+0x428b] = 1`) läuft. Läuft es durch den normalen Zweig, holt der die
ND-Stellung mit `0x5356c688` aus dem Einstellungsobjekt, das in M nie
aktualisiert wird. v3 hängt zusätzlich dort an:

| Stelle | Änderung |
|---|---|
| `0x5301a818` | Getter-Stub, 64 Bytes (Höhle reicht bis `0x5301a878`): ruft `0x5356c688`, ersetzt im Video `[r1]` durch MovieNdFilter (1 → 1, sonst 0) |
| `0x534543d0` | normaler Zweig: `bl 0x5356c688` → `bl` Getter-Stub |

Unicorn: 6 Fälle (Video an/aus mit veraltetem Einstellungsobjekt, Foto
beide Werte, kein UserData, Getter mit `r2 = 0`), r4–r11, sp erhalten.
`LOOK.PAT`: 11 Bereiche, 327 Bytes, danach `723382d6`, SHA-256
`ce052e2e…`.

Was Lauf 8 entscheidet: folgt `ae+5558` (Byte `+0x5558`) in Video M dem
Feld `ud+530`, kommt die Stellung im AE an; dann `+0x555a`, `9f0d`,
`nd_state` wie in P — oder die Kette reißt hinter dem AE.

### Lauf 8 (2026-10-22): `ndm` v3 — **ND im Video-M funktioniert**

`LOAD.OK` (danach `723382d6`). Nutzer: in Video M rastet der ND ein.
Log (Dateien `work/ramload/nd1/run8/`): jede Änderung von MovieNdFilter
(`ud+530`) erscheint nach ~35 ms in `ae+5558` (mit `+0x555a` = 1), nach
weiteren ~35 ms fährt der Treiber (`nd_state`), z. B. 10,598 → 10,637 →
10,682 s. Beim Verlassen des Videos (`ud+44` 1 → 0 bei 32,49 s) fährt der
Filter wieder heraus (Foto-Einstellung aus). Damit ist der normale Zweig
von `0x53454288` der Weg in Video M: der Getter-Stub (`0x534543d0`) ist
die wirksame Änderung; der Stub im Zweig „feste Belichtung“
(`0x5345431c`) bleibt als Absicherung.

Offen: Belichtung in M nach dem Einrasten (sollte um die ND-Stufe dunkler
werden, ohne dass Zeit/Blende/ISO sich ändern); `ndm` gehört noch nicht
zum Standardpaket `MANUAL` und ist nur als RAM-Patch getestet, nicht
geflasht.
