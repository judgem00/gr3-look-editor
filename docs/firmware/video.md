# Video der GR III — Pipeline, Encoder und ein „flaches" Profil

Firmware `fwdc239b`, statische Analyse (Capstone), Stand 2026-10-13.
Frage: Lässt sich ein möglichst rohes Videoprofil bauen, das mehr Spielraum
für Color Grading lässt?

## Kurzfassung

- **Echtes RAW-Video gibt es nicht.** Video läuft durch die
  Hardware-ISP (R2Y-Kern) und dann in einen H.264-Encoder auf einem eigenen
  Codec-Kern. Am Ende stehen 8-Bit-YCbCr und H.264. Die Debug-Shell kann
  zwar Einzelbilder als `C:\MOVIE%04d.raw` / `C:\Mov%04d.yuv` ablegen, das
  ist aber kein Aufnahmeweg.
- **Video nutzt dieselbe Bildpipeline wie Fotos.** Die
  `ImageTuningFactory` (`0x536f0308`) legt drei Instanzen derselben Klasse
  an: `m_ImageTuningStill`, `m_ImageTuningLiveview`, `m_ImageTuningAeSensor`.
  Der Liveview-Dispatcher (`0x536ddecc`, Vtable `0x53dbd938` Slot 7) ruft
  den Setting-Befüller `0x536dc0f4` (Look-Tabellen `0x55018d94` usw.) und
  dieselben Stufen auf wie der Foto-Pfad: ColorMatrix `0x536df3d4`,
  MultiAxial, YCbCrMatrix, Gamma `0x536e9000`, ToneControl, EdgeEnhancement,
  SuppreFilter … **Alles, was der Look-Editor patcht (Matrizen, Basiskurven,
  Kanalkurven), wirkt also auch im Video**, solange der Look im Videomodus
  wählbar ist.
- **Die Bitrate ist niedrig und patchbar.** 24p und 30p bekommen nur
  16,8 bzw. 21 Mbit/s, 60p dagegen 42 Mbit/s. Die Werte stehen als
  Immediate-Konstanten in einer kleinen Funktion (siehe unten).
- **Bester Weg zu einem flachen Profil:** Cross Process als Träger (im
  Videomodus vorhanden: `MovieFwupImageControlNo*ParamCrossProcess…`),
  Kanalkurven als flache Log-ähnliche Kurve, dazu eine niedrigere Sättigung
  über die Look-Matrix. Die Foto-Looks bleiben unberührt.

## Looks im Videomodus

Aus den UserData-Schlüsseln (`MovieImageControlNo0Param…`):
Standard, Vivid, Monotone, Hard Monotone, High Contrast B&W, Positive Film,
Bleach Bypass, Retro, dazu per Firmware-Update **Cross Process**.
Soft Monotone und HDR-Cuisine sind im Video gesperrt (Assert-Strings bei
`0xb30c18`). Pro Look gibt es die Regler Sättigung, Farbton, Key, Kontrast,
Kontrast Lichter/Schatten, Schärfe und Shading. Daneben gibt es eigene
Videoeinstellungen für `MovieShadowCorrect`, `MovieHighlightCorrect`,
`MovieColorSpace`, `MovieLensShadingCorrection` und `MovieNdFilter`.

## Encoder und Bitrate

Kette (alles ARM, RTOS):

```
MovieService           0x5366b038  baut Setup-Struct (fp-0x74)
  Bitrate-Ziel          0x5366a3d4(obj, Modus, fps)   -> Struct +0x24
  Bitrate-Max           0x5366a478 = 1,5 × Ziel       -> Struct +0x20
  fps aus Index         0x5366a5bc: 0→60, 1→50, 2→30, 3→25, 4→24
H264 Encode-Setup      0x5365b2f8  übersetzt in Encoder-Parameter
RB_InitParam_Set_Encode 0x5376167c übergibt an den Codec-Kern
                                   (inkl. 6×64 + 2×256 Byte Skalierungslisten)
```

`0x5366a3d4` (Einheit vermutlich kbit/s):

| Modus | fps ≤ 30 | fps > 30 |
|---|---|---|
| 0 (vermutlich Full HD) | 21000 · fps/30 | 42000 · fps/60 |
| 1 | 400 · fps | 21000 · fps/60 |
| 2 | 266,7 · fps | 21000 (fest) |

Für Modus 0 ergibt das:

| fps | Ziel | Max (×1,5) |
|---|---|---|
| 60 | 42,0 Mbit/s | 63,0 Mbit/s |
| 50 | 35,0 | 52,5 |
| 30 | 21,0 | 31,5 |
| 25 | 17,5 | 26,25 |
| 24 | 16,8 | 25,2 |

Pro Bild bekommt 24p also genauso wenig Daten wie 60p (0,7 Mbit). Für
Grading ist gerade 24p damit knapp.

**Patchstellen** (ARM-Immediates, je `movw`):

| Adresse | Befehl | Bedeutung |
|---|---|---|
| `0x5366a3e0` | `movwhi r3, #0xa410` | 42000, Modus 0, > 30 fps |
| `0x5366a3e8` | `movw r3, #0x5208` | 21000, Modus 0, ≤ 30 fps |
| `0x5366a43c`, `0x5366a42c` | `#0x5208` | Modus 1 / Rückfallwert |

Plausibilität gegen echte Clips steht noch aus (ffprobe/MediaInfo an
einer 24p- und einer 60p-Datei).

**Risiko gestaffelt:**

1. **24p/30p verdoppeln** (`0x5366a3e8`: `#0x5208` → `#0xa410`, also
   21000 → 42000). Ergebnis: 30p 42/63 Mbit/s, 24p 33,6/50,4 Mbit/s. Das
   bleibt innerhalb dessen, was Encoder, Puffer und Karte bei 60p ohnehin
   leisten. Geringes Risiko, ein Befehl.
2. **Über 63 Mbit/s Spitze** (z. B. 60p auf 60–80 Mbit/s): offen sind die
   Größe des Movie-Puffers (`MovieBufferManager`, Meldung
   `[MP] __DROP FRAME`), die Schreibrate der Karte, das H.264-Level im
   Header und mögliche Grenzen im Codec-Kern selbst. Nur schrittweise und
   mit Testaufnahmen bis zum Kartenende.
3. Die Restzeitberechnung (`CMD_MOVIE_CALC_AVAILABLE_SEC_MOVIE`) wird mit
   höherer Rate falsch, solange sie eigene Konstanten nutzt. Kosmetisch.

QP-Grenzen stehen im Encode-Setup als Konstanten (`0x33` = 51 an
Param `+0x18` und `+0x28`, `0x2a`, `0x190`); ihre Bedeutung ist nicht
belegt. Nicht anfassen, solange die Bitrate reicht.

## Ein flaches Profil

Ziel: möglichst viel Dynamik und neutrale Farbe in 8 Bit retten, ohne
Banding. Ein echtes Log wäre bei 8 Bit 4:2:0 und ~40 Mbit/s zu steil in den
Mitteltönen; besser ist eine „Cine"-artige Kurve: Schatten leicht
angehoben, weite Lichter-Schulter, Mitteltöne fast linear.

Bausteine, alle schon im Werkzeugkasten:

| Stufe | Werkzeug | Einstellung |
|---|---|---|
| Tonkurve | Cross-Process-Kanalkurven (`0x55037604`, Preset 1–3) | Für alle drei Kanäle dieselbe Kurve C = Ziel ∘ Basis⁻¹, damit die feste Basiskurve neutralisiert wird |
| Look-Daten | Zeigertausch Eintrag 21 → neutraler Datensatz | Matrix eines neutralen Looks (Standard), nicht Negative Film |
| Sättigung | Look-Matrix bzw. Regler | Regler −1…−2, nicht tiefer (Sättigung lässt sich in 8 Bit schlecht zurückholen) |
| Schärfe | Regler | −4; Nachschärfen geht besser in der Post |
| Schatten-/Lichterkorrektur | Videomenü | aus, damit die Kurve nicht pro Szene wandert |
| Bitrate | Code-Patch oben | 24p/30p mindestens auf 60p-Niveau |

Die Kanalkurven haben 256 Stützstellen über den 14-Bit-Ausgang der
Basiskurve (`Basis >> 6`). In den Schatten ist die Basiskurve steil; dort
reicht die Auflösung. Ein LUT für die Post (Rec.709-Rücktransformation)
lässt sich direkt aus der verwendeten Kurve berechnen.

## Was offen ist

1. **Greift Cross Process im Video?** Der Gamma-Stufe ist der Modus egal,
   aber ob der Cross-Process-Koordinator (`0x536ef140`) im Liveview-Pfad
   läuft, ist nicht belegt. Test ohne neuen Flash: Auf der schon
   geflashten Firmware mit eigenen RGB-Kurven einen kurzen Clip mit Cross
   Process Farbton 1–3 aufnehmen.
2. Bitrate-Einheit und Modus-Zuordnung an echten Clips bestätigen.
3. Bildbereich: ob die Kamera Full Range oder 16–235 schreibt und wie die
   MOV-Datei das kennzeichnet (CSP-Grenzen in `YCbCrOperation`).
4. Rauschfilter im Video (`LumaNr`, `SuppreFilter`) folgen ISO-Tabellen;
   nicht untersucht.
5. **ND im Video-M** — ruht (2026-10-16): drei Anläufe (`nd`, `ndprobe`,
   `ndaction`) ohne Wirkung, siehe Abschnitt „ND-Filter im Video-M" (dort
   auch die offenen Spuren).

## Manuelle Belichtung im Video (Stand 2026-10-13, unvollständig)

Frage: Lässt sich M (oder Tv/Av) für Video freischalten? Video belichtet
heute nur mit Programmautomatik und Belichtungskorrektur.

Belegt:

- **Die AE-Engine kann M.** Der Shell-Befehl `AE_Mode` (`0x5301a734`) kennt
  `P -p`, `Av -a`, `Tv -t`, `Sv -s`, `M -m` und schreibt den internen Modus
  (P = 0, Tv = 1, Av = 2, Sv = 3, M = 5) über `0x53491310` (Feld `+0x8e`)
  und `0x5349176c` (`+0x122`).
- **Liveview und Video nutzen dieselbe AE-Rechnung.** `AeLiveViewPipeService`
  hält `m_AeSwitchDial` und `m_AeProgramDiagram`; Video ist ein Zustand des
  Liveview-Pfads (`Liveview Movie start`).
- **Die Moduswahl kommt über das CommonProperty.** `0x5348e21c` liest den
  Modus aus `CommonProperty+0x74f` (Getter `0x5354999c`), ein Flag aus
  `+0x74e`, und bildet beides in `0x5348dfbc` auf den internen Modus ab.
- **Belichtungswerte sind gemeinsam.** Die UserData-Schlüssel `CommonTv`,
  `CommonAv`, `CommonSv` (Index 0–2) gelten für Foto und Video. Einen
  Belichtungsmodus gibt es nur als `StillExposureMode` (0x36); für Video
  existieren nur `MovieAeMode` (Messmethode), `MovieSvAuto`,
  `MovieSvAutoUpper/Lower`, `MovieSvAutoLowerTv`.
- `AeModel::UpdateExposureMode` (App-Seite, `0x532241ac`) bildet die
  Moduswahlrad-Position (3…12) auf den Belichtungsmodus ab.

**Die Sperre (gefunden):** `CreateExposureMode` (`0x5327128c`, App-Seite,
Name aus dem Assert-String) berechnet den Modus, den die App als
Property `0x99` an den Kern schickt (`SyncExposureMode` `0x53273334` →
`0x5354be24` → Kern-Setter `0x5355e60c` → `CommonProperty+0x74f`):

```
5327129c  ldr   r3, [r0, #4]        ; UserData
532712a4  ldrb  r2, [r3, #0x44]     ; CommonStillMovie (Getter 0x53243e40)
532712a8  cmp   r2, #1              ; 1 = Movie
532712ac  beq   0x53271314          ; -> return 0 (P)
532712b0  ldrb  r4, [r3, #0x142]    ; StillExposureMode (Stellung Moduswahlrad)
```

Patch: `beq` → `nop` (`0x532712ac`: `1800000a` → `00f020e3`). Dann gilt
im Video der Modus des Wahlrads. ([`tools/video-test.py`](../../../gr3x-fw-hack/tools/video-test.py)
`build manual`.) Andere Stellen, die im Video eigens abzweigen, betreffen
Bedienelemente (z. B. `0x532713a4`, `0x532713f8` geben im Video 0 zurück).

Nicht belegt, nur am Gerät prüfbar:

- ob der Kern im Videobetrieb M wirklich umsetzt oder selbst noch klemmt
  (Liveview-AE, Programmdiagramm für Video);
- welche Zeit/Blende/ISO er dann nimmt. Erwartet: die gemeinsamen
  `CommonTv/Av/Sv` aus dem Foto-M;
- ob die Video-Oberfläche mit einem anderen Modus als P an einem Assert
  hängen bleibt. Ein fehlschlagender Assert (`0x536b93d0`) protokolliert
  und hält wahrscheinlich an; dann hilft Akku raus.

Weitere Hürden, auch wenn der Modus durchgeht:

- **Bedienung:** Im Videomodus sind die Rädchen auf Belichtungskorrektur
  belegt. Weil Tv/Av/Sv gemeinsame Werte sind, ließen sich die im Foto-M
  eingestellten Werte übernehmen. Ändern ginge dann nur über den Fotomodus.
- **Grenzen:** Belichtungszeit höchstens 1/fps (Sensor im Video-Auslesemodus),
  ISO im Bereich von `MovieSvAuto…`; Blendenwechsel während der Aufnahme
  sind hörbar und springen.

### Test 1 (2026-10-13): Wahlrad-Modus im Video

Ergebnis an der Kamera mit `build manual`: Das Video zeigt jetzt den Modus
des Wahlrads an, belichtet aber weiter wie P. Die App-Sperre war also nur
die erste Hürde.

Befund im Kern (Ghidra über REA, Fenster `0x5346a000…0x534a0000`):

- Der Modus kommt an: Property `0x99` → `CommonProperty+0x74f` →
  AE-Feld `+0x8e` (`0x5348e8f4`). Das Foto/Video-Flag reist im selben
  Block mit (`+0x74e`, App-Struct `+0x8a`) und landet als AE-Flag `+0x3a`
  (Getter `0x53491078`).
- Die Liveview-AE wählt pro Bild eine **Programmlinie** (`+0x150d`,
  Auswahl in `0x53471158`). Im Video gibt es eigene Zweige: M → Linie
  `0x0c`/`0x0e`, Modus 4 → `0x0b`, Tv → `0x12`, Av → `0x07`.
- Die Linien werden in `0x5347a9b0` ausgeführt. **Alle sind Regelkreise
  auf die gemessene Helligkeit.** `0x0c` ruft `0x534762d8` mit Variante 4
  auf (feste Blende aus `ctx+0x174`, Zeit und Verstärkung automatisch),
  `0x11`/`0x12` rufen `0x534778f0` auf (ebenfalls Regelung). Keine Linie
  übernimmt die eingestellten Tv/Av/Sv.
- Die eingestellten Werte (Getter `0x5349199c`, `0x53491984`,
  `0x534919b4`, `0x53491a44`) liest nur der Startzweig von `0x5346fb2c`
  (Flag `+0x1585`, gesetzt beim Liveview-Start in `0x5346d334`, danach
  gelöscht). Wie das Foto-M seine Werte danach im Sucherbild hält, ist noch
  nicht geklärt.

Folgerung: Ein echtes M im Video braucht einen Eingriff in die Liveview-AE,
nicht nur einen Schalter. Infrage kommt, das Video bei M in den Pfad
umzuleiten, der die festen Werte setzt, und die Regelung auszusetzen.

### Patch `mhold` (Test 2, noch nicht geflasht)

Weitere Befunde:

- Die AE-Ereignisschleife `0x5346c810` ruft pro Bild (Ereignis 1)
  `0x53472e30` → `0x53472520` auf: Linienauswahl `0x53471158`, dann
  Regelung `0x5346db18`. Ereignis 2 ruft `0x5346fb28` auf; dort rechnet
  der Startzweig (Flag `+0x1585`, gesetzt in `0x5346d334`) bei Modus 5/8
  die Belichtung aus den eingestellten Tv/Av/Sv.
- `0x53472520` hat einen **Halte-Zweig**: Ist im AE-Datenblock `+0xa7`
  gesetzt (`0x53472628: ldrb sl,[r5,#0xa7]; cmp; bne 0x534728b4`), wird
  nicht geregelt, nur die aktuellen Sensorwerte zurückgelesen
  (`0x5347aeb4`). Vermutlich der AE-Lock.

Patch: Die Abfrage wird zum Aufruf einer Hilfsroutine. Diese meldet
„halten“, wenn `+0xa7` gesetzt ist oder (Video-Flag `info+0x3a` und Modus
`info+0x8e == 5`). Im Fotobetrieb ändert sich nichts. Die Routine
überschreibt den toten Debug-Shell-Handler `AE_Mode` (`0x5301a730`, keine
Aufrufe, keine Zeiger). Ein erster Versuch im leeren Bereich
`0x53003f00` wurde verworfen: Dahinter liegt eine Datentabelle, die
Ausführbarkeit ist nicht belegt.

Mögliche Ergebnisse:

| Beobachtung im Video bei M | Bedeutung |
|---|---|
| Belichtung folgt Tv/Av/ISO und bleibt fest | Startzweig läuft beim Videostart: echtes M |
| Belichtung friert auf einem Automatikwert ein | Halten wirkt, der Startzweig läuft aber nicht beim Wechsel (AE-Lock-Verhalten) |
| Belichtung regelt weiter | Video-Flag oder Modus ≠ 5 im Kern |

### Test 2 (2026-10-13): `mhold` ohne Wirkung → Patch `mlive`

Ergebnis an der Kamera: Mit `mhold` regelt die Belichtung im Video bei M
weiter. Also war mindestens eine Bedingung falsch: Video-Flag `+0x3a`
nicht gesetzt, oder der Kern hat nicht Modus 5 (bei ISO AUTO macht
`0x5348e8cc` aus M den Modus 4).

Weitere Befunde:

- `0x53472e30` (Ereignis 1) hat einen zweiten Pfad: Ist
  `**(this+0x1478)` gesetzt, läuft statt der Regelung `0x5346f850`, eine
  **feste Belichtung** aus einem Struct (Belichtungszeit `+4`, weitere Werte
  `+8`, `+0xc`, Flags `+0x10`/`+0x11`). Befüllt wird es über die
  AE-Kommandos `SetLiveViewExposureManual` / `…Iso` / `…IntegralTime` /
  `…Aperture` (Dispatcher `0x53496124`, Kommandos `0x15`…`0x1a` aus
  `msg->[4]+0xaf9`). Wer sie sendet, ist offen. Kein Treffer im Linux-Teil.
- Bei gesetztem `+0xa7` übernimmt `0x53472e30` fertige Werte aus dem
  AE-Datenblock (`+0xec`…`+0xfc`).

Patch `mlive`: Der Aufruf pro Bild in der Ereignisschleife
(`0x5346c84c`) geht über eine Hilfsroutine. Bei Modus 5 setzt sie das
Startflag `this+0x1585` und springt nach `0x5346fb28`. Der Startzweig
rechnet die Belichtung aus Tv/Av/Sv, jetzt bei jedem Bild. Ohne
Video-Prüfung, damit der Test auch im Foto-Liveview etwas zeigt.

| Beobachtung | Bedeutung |
|---|---|
| Video M: dunkel und fest | echtes M im Video |
| Video regelt weiter, Foto-Liveview bei M zeigt jetzt die eingestellte Belichtung | Modus 5 kommt im Foto an, im Video nicht (ISO AUTO / Modus 4?) |
| Nirgends eine Änderung | Haken nicht erreicht oder Modusnummer ≠ 5 |

### Test 3 (2026-10-13): `mlive` → Patch `mlive2`

Ergebnis: Video bei M regelt weiter. Foto-M-Liveview wie mit der
Original-Firmware (die zeigt die eingestellte Belichtung ohnehin), aber
der **Autofokus im Foto geht nicht mehr**. Das zeigt, dass der Haken im
Foto-M erreicht wird (Modus 5 kommt an); im Video wirkt er nicht.

AF-Ursache (vermutet): Der Startzweig setzt `p3+0x55 = 0`, die
Fest-Belichtung `0x5346f850` meldet dagegen `aeData+0x118 = 1`,
`p3+0x55 = 1` (AE eingeschwungen).

Patch `mlive2`: Auslöser ist Video-Flag `info+0x3a` **oder** Modus 5.
Nach `0x5346fb28` meldet der Stub „eingeschwungen“.

| Beobachtung | Bedeutung |
|---|---|
| Video in jedem Modus fest belichtet, bei M nach Tv/Av/ISO | Lösung (P im Video ist in diesem Test absichtlich fest) |
| Video fest, aber bei M nicht nach den Werten | Video-Flag stimmt, Modus im Video ≠ 5 |
| Video regelt in jedem Modus weiter | Video-Flag falsch, oder Ereignis 1 läuft im Video nicht |
| Foto-AF bei M wieder in Ordnung | AF-Ursache bestätigt |

### Test 4 (2026-10-13): `mlive2` → Patch `mlive3`

Ergebnis: Video bei P und M fest belichtet, M aber nicht nach den
eingestellten Werten. Foto-AF geht wieder, aber es lässt sich kein Foto
auslösen.

Folgerungen:

- Video-Flag `info+0x3a` stimmt, und das Video nutzt denselben Weg pro Bild
  (Ereignis 1, `0x5346c84c`).
- Im Video steht in `info+0x8e` nicht 5, deshalb nimmt `0x5346fb28`
  Standardwerte. Vermutete Ursache: `0x5348e39c` aktualisiert `+0x8e` nur bei
  `+0x156 == 0`; der Video-Zweig von `0x5348dfbc` schreibt den Modus
  stattdessen nach `+0x122`.
- Die AF-Meldung („eingeschwungen“) war die AF-Ursache. Dass das Auslösen
  blockiert, liegt am Eingriff im Fotobetrieb; `mlive3` lässt Fotos
  unberührt.

Patch `mlive3`: nur bei Video-Flag; wenn `+0x122 == 5`, wird `+0x8e = 5`
gesetzt und `0x5346fb28` gerufen, danach „eingeschwungen“ gemeldet. Video
bei P und Fotos bleiben original.

### Test 5 (2026-10-13): `mlive3` → Patch `mlive4`

Ergebnis: Fotos (AF, Auslösen) normal, Video bei P regelt normal, AF im
Video bei M geht. Video bei M ist fest, aber **immer dunkel**. Eine im
Foto-M eingestellte Belichtungszeit wird nicht übernommen, und im Video
lässt sie sich nicht einstellen.

Ursache: `0x5346fb28` liest bei M nicht die eingestellten Werte, sondern
die Ausgänge des Programmdiagramms: Tv `+0x166`, Av `+0x168`, Sv `+0x16a`.
Diese Ausgänge schreibt der Verteiler `0x534869a8` (M →
`0x534868f0`, Quelle Tv `+0x128`/Av `+0x12a`), der im Video offenbar nicht
neu rechnet. Die eingestellten Werte stehen in Sv `+0x19a`, Tv `+0x19c`,
Av `+0x19e` (Property-Übernahme `0x5348f824`). Im Video begrenzt
`0x534916c4` zusätzlich die Zeit (`*(this+0x30)` statt `0x1c`).

Patch `mlive4`: wie `mlive3`, kopiert aber vorher die eingestellten
Sv/Tv/Av in die Diagramm-Ausgänge. Ungeprüft: ob die Einheiten von Sv in
`+0x19a` und `+0x16a` gleich sind.

### Nachtrag Test 5: Foto-M hängt nach dem ersten Bild → Patch `mlive5`

Mit `mlive3` funktionieren Foto-P und das **erste** Foto in M normal. Danach
reagiert der AF nicht mehr, und die Kamera löst nicht aus. Folgerung:
`info+0x3a` (aus `CommonProperty+0x74e == 1`) ist nach einer Aufnahme auch
im Foto-M gesetzt. Es ist also kein reines Video-Flag; seine genaue
Bedeutung ist offen. Der Stub greift dann und umgeht die AE→AF-Kopplung von
`0x53472e30` (`this+0x147c`, `0x5346f9fc`, Meldung `0x40d0005`).

`mlive5` = `mlive4` + Schutz: Solange die AE→AF-Kopplung aktiv ist
(`**(this+0x147c) != 0`), läuft die normale AE. Ungeprüft ist, ob die
Kopplung beim Foto-AF wirklich gesetzt wird. Zeigt der Test weiter
Hänger im Foto-M, braucht es ein anderes Video-Merkmal.

### Test 6 (2026-10-13): `mlive5` → Patch `mlive6`

Ergebnis: Foto-M funktioniert jetzt (AF, mehrere Aufnahmen). Der AF-Schutz
über `this+0x147c` wirkt. Video bei M **bringt die Kamera zum Absturz**.

Vermutete Ursache: die Sv-Kopie `+0x19a → +0x16a`. In `0x5346fb28` geht
Sv als `(Sv & 0xff) + 8` in eine Belichtungsstufe ein, die als
Tabellenindex mit Verschiebung (`DAT_53c5419c[x & 7] << ((x-8)>>3)+1`)
in eine Division `256000000 / …` läuft. Bei falscher Einheit kann der
Divisor 0 werden. `mlive3` (ohne Kopie) stürzte nicht ab.

`mlive6`: kopiert nur Tv/Av, und zwar aus `+0x128`/`+0x12a` (Quelle des
M-Diagramms, gesetzt in `0x5348f824` aus `+0x19c`/`+0x19e`); kopiert
nicht, wenn einer der Werte 0 ist. ISO bleibt beim zuletzt berechneten
Diagrammwert. Hinweis: `+0x128` ist derselbe Wert wie `+0x19c`. War die
Tv/Av-Kopie die Absturzursache, stürzt auch `mlive6` ab.

### Test 7 (2026-10-13): `mlive6` stürzt ebenfalls ab → Patch `mlive7`

Ergebnis: Auch `mlive6` (nur Tv/Av, Nullschutz) bringt die Kamera im Video
bei M zum Absturz. Die Einheiten sind dabei nicht das Problem: Der Kern
schreibt in `0x53482f80` (`0x53483020…64`) und `0x5347f79c` jeweils
denselben Wert in Einstellungs- und Diagrammfeld (Tv `+0x128`/`+0x19c`,
Av `+0x12a`/`+0x19e`, Sv `+0x16a`/`+0x19a`). Der Absturz entsteht beim
Aufruf von `0x5346fb28` im Videobetrieb; der genaue Grund ist offen
(Kandidat: Division `256000000 / (tab[x&7] << …)` mit Divisor 0).

Neuer Ansatz: Foto-M zeigt mit Originalfirmware die eingestellte
Belichtung im Liveview, auch nach einer Aufnahme mit `+0x3a = 1`, also über
die Video-Zweige der AE. Im Video fehlt nur der Modus in `+0x8e`.
`mlive7` setzt im Video `+0x8e = 5`, wenn `+0x122 == 5`, und lässt dann
die unveränderte AE laufen.

### Test 8 (2026-10-13): `mlive7` funktioniert

Ergebnis: Video bei M belichtet nach den im Foto-M eingestellten Werten.
Einschränkung: Die Werte lassen sich nur im Foto-M einstellen; danach wird
ins Video gewechselt.

Stand des Patches (`tools/video-test.py build manual mlive7 [bitrate]`):

| Stelle | Änderung |
|---|---|
| `0x532712ac` | `CreateExposureMode`: Video erzwingt nicht mehr P |
| `0x5346c84c` | AE-Ereignisschleife ruft pro Bild den Stub statt `0x53472e30` |
| `0x5301a730` | Stub (über totem Shell-Handler `AE_Mode`): im Video `+0x8e = +0x122`, wenn M |
| `0x5366a3e8` | optional: Bitrate ≤ 30 fps verdoppelt |

Offen: Bedienung im Video (Zeit/Blende/ISO an den Rädern), Verhalten bei
Av/Tv im Video, Bitrate an echten Clips prüfen.

### Bedienung im Video (Stand 2026-10-13, Anfang)

Die Tastenfunktionen der Aufnahmebereitschaft (`CaptureReadySwitchFunctionModel`,
Handler `0x53297e54`…`0x532999f4`) schicken ihre Aktion
(`0x53207c8c(msg, ID)` → `0x53200594`) nur, wenn
`GetCommonStillMovie() != 1`. Im Video sind sie also stumm. Beispiele für IDs:
`0x95`, `0x97`, `0xbf`, `0xc0`, `0xb9`, `0xba`, `0x98`, `0x45`, `0x9b`,
`0x9c`, `0x78`, `0x79`, `0x91`, `0x92`, `0xc1`, `0xbd`, `0x6a`; `0x6c`
nur bei Modus 5. Welche ID Zeit/Blende/ISO an den Rädern ändert, ist
noch nicht zugeordnet. Die Räder selbst (`FDIAL`, `RDIAL`, `UpDownDial`,
`AdjLeft/Right`) laufen über einen anderen Weg, der noch gesucht wird.

### Patch `dials` (Test 9, noch nicht geflasht)

Die Räder der Aufnahmebereitschaft laufen über drei Handler in
`CaptureReadySwitchFunctionModel` (dekompiliert). Sie wählen die Aktion
nach dem Belichtungsmodus (`CommonProperty+0x74f`):

| Handler | Foto | Video (original) |
|---|---|---|
| `0x5329512c` | P: Shift `0x40/41`, Tv: `0x38/39`, Av: `0x3a/3b`, M: Tv oder Av (`0x53246174`) | bricht ab (`0x532952cc`) |
| `0x532957cc` | P/Tv/Av: Korrektur `0x3e/3f`, M: Av oder Tv | immer Korrektur (`0x5326c1b4` = Video) |
| `0x53295da0` | ISO `0x51/52`, … | bricht ab (`0x53295e84`) |

Patch `dials`: die Video-Abbrüche in Handler 1 und 3 entfernen, in
Handler 2 die Abkürzung zur Korrektur überspringen. Video bei M sollte dann
Zeit/Blende/ISO an den Rädern ändern. Video bei P: Handler 1 schickt eine
Programmverschiebung, deren Verarbeitung im Video ungeprüft ist.

### Test 9 (2026-10-13): `dials` funktioniert

Ergebnis: Im Video bei M stellen die Räder Zeit und Blende ein, die
Belichtung folgt. Offen: Die Einstellungen erscheinen nicht im Overlay.

### Overlay (Stand 2026-10-13, Anfang)

Die Anzeige liest das Foto/Video-Flag über den CommonProperty-Getter
`0x53549958` (`+0x74e`). `0x532ab430` blendet ein Element aus, wenn das
Flag gesetzt ist. Die drei Aufrufer `0x531c515c`, `0x531c5268` und
`0x531c5318` zeichnen aber die ISO-Auto-Elemente (UserData `+0x148`,
`+0x14c`, `+0x154` = StillSvAuto, …Upper, …LowerTv), nicht Zeit und
Blende. Die Elemente für Zeit und Blende sind noch nicht gefunden. Weitere
Aufrufer von `0x532ab430`: `0x53173c98`, `0x53175b58`, `0x5319a308`.

### Patch `osd` (Test 10, noch nicht geflasht)

Weg zum Overlay: Formatierer `1/%d` → `0x532b5e58` (Zeit), `F%d.%d` →
`0x532b6034` (Blende). Aufrufer sind die Overlay-Elemente `0x5317bb84`
(Zeit) und `0x5317bca0` (Blende) im Verteiler `0x5317ca24`, der alle
Liveview-Elemente der Reihe nach aktualisiert. Beide sind nur sichtbar,
wenn `0x5317904c` wahr liefert: Controller-Zustand (`0x5316d8d4`) ==
`0x4000001` und Anzeigestufe `this+0x306` ∈ {1, 2}. Ein zweiter
Verteiler `0x53182324` zeichnet dieselben Werte bei Zustand `0x4000007`.
Videoelemente wie die Aufnahmezeit (`0x531714c8`) prüfen den Zustand
nicht, sondern `*(this+0x4e8)+0x940`.

Patch `osd`: Die beiden Aufrufe von `0x5317904c` gehen über einen Stub
(`0x5301a760`, toter `AE_Mode`-Code hinter `mlive7`): im Video (UserData
StillMovie == 1) sichtbar, sonst Originalbedingung. Offen ist, ob der
Verteiler `0x5317ca24` im Video überhaupt läuft.

## ND-Filter im Video-M (Stand 2026-10-16)

Beobachtung (Kamera, Firmware mit `manual mlive7 dials osd bitrate`): Im
Video bei P rastet der ND-Filter hörbar ein. Bei M erscheint nach Druck
auf die ND-Taste das ND-Symbol, aber der Filter bewegt sich nicht, und das
Bild wird nicht dunkler. Bei P passiert mit denselben Patches alles wie ab
Werk (der `mlive7`-Stub greift nur, wenn der Videomodus M ist), der
Unterschied ist also der **Belichtungsmodus M im Video**.

### Der AE-Befehl und seine Kennung

Die AE-Befehle sind eine gemeinsame Struktur (im Folgenden `cmd`), die per
Nachricht `0x46420101` an alle AE-Teile geht:

| Feld | Bedeutung |
|---|---|
| `cmd+8+4n` | Wert des n-ten Liveview-Settings (`0x53583a00/0x53583a0c`) |
| `cmd+0xaf8` | Klasse (2 = Liveview-Settings, 0x15 = zweite Familie) |
| `cmd+0xaf9` | Kommando in der Klasse (2 → 0x15…0x1a) |
| `cmd+0xafb` | Sperrbyte, 0 = durchlassen, 1 = verwerfen |
| `cmd+0xafc` | Status, den der Empfänger zurückschreibt (0 ok, 1 ?, 3 unbekannt) |

- Empfänger im Kern: `0x534965c0` liest `+0xaf8` (2 oder 0x15) und geht in
  den Dispatcher `0x53496124` (Klasse 2: Kommandos 0x15…0x1a); für
  `af9 = 0x1a` läuft der Handler **`SetLiveViewNdFilter`** `0x53495ff4`
  (String `0x53c57be0`). Er liest den Wert mit `0x53583a0c(cmd, 0)` =
  `[cmd+8]` und schreibt ihn nach `[[ctx+8]+0x1478]+0x10`, das Struct der
  festen Liveview-Belichtung (gelesen von `0x5346f850`, pro Bild in die
  Belichtungsausgabe `+0x90…+0x93`).
- Vor dem Dispatch prüft `0x533baeb8` die Kombination `(af8, af9)` gegen
  die Tabelle **`0x53c0fdc4`** (3-Byte-Einträge `{Klasse, Kommando,
  Kennzeichen}`, Ende 0xff; ND = `{2, 0x1a, 8}`): bekannte Kombination →
  `+0xafb = 0`, unbekannt → `1`.
- Der Empfänger im App-Teil ist `0x533bc208` (Klasse 0xf); die
  Senderseite baut die Nachricht um `0x533ba96c`. Die App wertet die
  ND-Einstellung also zweimal aus: als Property-Schreibweg (unten) und als
  AE-Befehl über `0x533bb800` (liest `0x5356d26c`, negiert den Wert,
  schreibt `[cmd+8]`; läuft nur bei `[Schnappschuss+0x38] == 1`, sonst
  `afc = 1`).

### StillNdFilter und MovieNdFilter

- Zwei UserData-Felder: `+0x172` **StillNdFilter** (0 = Auto, 1 = On,
  2 = Off) und `+0x530` **MovieNdFilter** (1 = On, 2 = Off). Die
  Einstellungswächter `0x53346e40` (Schlüssel `0x53b23cdc`, `0x53b28108`)
  rufen die Setter `0x5324c010` (Still) bzw. `0x53253784` (Movie). Die
  „NdFilter“-Treffer bei `0x5332e6ec…0x533301bc` sind Einträge genau
  dieses Verteilers (Schlüssel „NdFilter“), keine Tastenbelegung.
- Beide Setter schreiben den Kern nur mit einer **Rückfrage**: sie lesen
  den ND-Zustand des Kerns (`0x5354d8ac(core, i)`, Property 0xa4 Index i)
  und schreiben die Property `0xa4` (`0x5354c02c`) nur, wenn „der Filter
  ist in einer anderen Variante drin“ gilt (`0x53253748` = Index 1 | 2 für
  Movie; `0x5324bfb4` = für jeden Index aus `0x53b590ac` außer
  `[core+0x769]` für Still). Zusätzlich prüft der Still-Setter den
  Video-Merker `[userdata+0x44]`: im Video schreibt er nie in den Kern,
  sondern meldet nur.
- Der Kern hält die ND-Varianten in `[[core+0x518]+0x768…]` (Getter-Familie
  `0x535514ec…`). Die Settings-Übertragung `0x5348dd04`, `0x5348de40`,
  `0x5348e21c` kopiert sie in das AE-Info (`+0x58`, `+0x49a`, `+0x659`);
  den Weg nach `+0x58` (`0x5348dd60`, Quelle `[core+0x76a]`) öffnet nur
  `[settings+0x35] != 0`, sonst rechnet der AE selbst (`0x53491fe0`).

### Wie der Filter fährt

- **Es gibt genau einen Absender** des FSS-Befehls `0x64c40802`:
  `0x53454a9c` (gerufen über `0x534449d4` aus dem Bedien-/Sequenzmodul,
  u. a. `0x534466a4` und `0x53448bf8`).
- Auf der anderen Seite fährt der Handler `0x5345d514` die Rampe und ruft
  den Treiber: `0x536f143c` = **ND-OPEN**, `0x536f15dc` = **ND-CLOSE**
  (die Shell `0x53046dd4`/`0x53046e1c` protokolliert „ND-OPEN.“ /
  „ND-CLOSE.“, Strings `0x54023854`/`0x54023860`).
- Das Bedienobjekt führt die Anforderung in `+0x730` (gesetzt von
  `0x53440154` aus dem Feld `[+1]` des Request-Structs, das `0x53436558`
  aus `[obj+0x49]` füllt). Die Fahrstufe ist nur freigegeben, solange
  `[obj+0x728] & 0x10` gesetzt ist (`0x534401b4`, gelöscht nach dem Fahren
  in `0x5344aaec`).
- Fazit: Der Filter fährt **nur**, wenn die Sequenz den ND-Auftrag
  bekommt, und der kommt aus `[cmd+8] == 1` (`0x53436738`).

### Patch `nd` (Test 11, 2026-10-16 geflasht — ohne Wirkung)

Ansatz: die App soll den ND-Zustand im Video **immer** an den Kern melden,
die Rückfrage vor dem Schreibweg weg. Zwei Stellen, je eine Instruktion:

| Stelle | Änderung |
|---|---|
| `0x5324C034` | Still-Setter: `beq` auf den Kern-Schreibweg (`0x5324c060`) wird unbedingt (`09 00 00 0a` → `09 00 00 ea`) |
| `0x532537F4` | Movie-Setter: Verfügbarkeitsprüfung `0x53253748` übersprungen (`beq 0x532537c4` → NOP) |

Build: `tools/video-test.py build manual mlive7 dials bitrate nd` (im
Nachbar-Repo `gr3x-fw-hack`, dort liegen die Werkzeuge), Basis
`unpackedFW-looks` aus `work/grlog-test`, Ausgabe
`work/nd-video-m/FWDC239B.BIN` (geprüft mit `fw-pack.py check`).

**Ergebnis am Gerät: keine Wirkung.** Video-M, ND-Taste: das Symbol kommt
(wie vorher), der Filter fährt nicht. Also liegt die Blockade **nicht** (oder
nicht nur) an der Rückfrage vor dem Kern-Schreibweg — die App-Seite ist nicht
der Auslöser. Wichtig für alles Weitere: die Taste ist nicht stumm geschaltet,
es fehlt allein der Antrieb.

### Patch `ndprobe` (Test 12, Sonde)

Weil der Zustands-Weg als Ursache ausfällt, beantwortet die Sonde zuerst die
Grundfrage: **fährt der ND-Antrieb im Video-M überhaupt?** Sie hängt sich an
den Einstellungs-Verteiler `0x53346e40` und stellt beide ND-Zweige vor ihren
Original-Handler:

| Stelle | Änderung |
|---|---|
| `0x5334CE34` | StillNdFilter-Zweig (`b 0x532df000`) → Stub A `0x5301A754` |
| `0x5334E388` | MovieNdFilter-Zweig (`b 0x53309540`) → Stub B `0x5301A760` |

Stub (je 8 Byte): `bl 0x53046e1c` — der Debug-Shell-Aufruf „ND-CLOSE."
(holt das ND-Objekt `0x536f0df0`, ruft den Treiber `0x536f15dc` mit `r1=1`)
— und danach `b` auf den Original-Handler. Damit fährt **jede**
ND-Einstellungs-Änderung den Filter ein, in Foto und Video.

Nebenwirkung, gewollt als Signal: der Filter wird nie wieder ausgefahren
(„ND aus"/„Auto" stellen nur die Einstellung um, der Filter bleibt drin) —
zurück only per Original-Firmware. Stub B liegt im `osd`-Bereich, die beiden
Patches schließen sich also aus (das Werkzeug bricht bei beiden ab).

Build: `tools/video-test.py build manual mlive7 dials bitrate ndprobe`
(ohne `nd`, ohne `osd`), Ausgabe `work/nd-probe/FWDC239B.BIN`, SHA-256
`2e1068313d1006a8f8b2b8128c58bfdcb17235106a8883de1cd75e0b6703764d`,
`fw-pack.py check` ok; die vier neuen Sprungwörter sind im gebauten Bild
mit Capstone rückgeprüft (Ziele Stub A/B, ND-CLOSE, Original-Handler).

Am Gerät zu prüfen: Video-M ND-Taste — rastet der Filter jetzt ein
(hörbar, Bild ~2 EV dunkler)? Video-P als Kontrolle (rastet ein wie immer);
Foto: ND ein/„aus" — bleibt der Filter beim „Aus" drin?

**Ergebnis am Gerät (2026-10-16): in Video-M rastet nichts ein.** Damit ist
der Einstellungs-Pfad (Bus + Handler) als Ort der Taste ausgeschlossen: im
Video-M läuft er gar nicht — die Taste läuft woanders, das Symbol kommt aus
einer anderen Quelle.

### Patch `ndaction` (Test 13)

Der Ort, an dem die Taste in Video-M wirklich läuft: die **Aktion der
ND-Funktionstaste** `0x533bb800` (ein Fall der Aktions-Tabelle um
`0x533bc2e4`; die Nachbarfälle behandeln die anderen belegbaren Funktionen,
jeder mit eigener „Zustand lesen"-Funktion — der ND-Fall liest `0x5356d26c`,
Property 0x85). Sie prüft das Geräte-Kennzeichen `[obj+0x38]` und geht dann
zwei Wege:

| Weg | Bedingung | tut | Stelle |
|---|---|---|---|
| A | Kennzeichen == 1 | Wert := !Kern-Zustand, schreibt über `0x53583a00` | `0x533bb848` |
| B | Kennzeichen != 1 | Wert := 1, schreibt über `0x53583a18` | `0x533bb82c` |

Beide Schreiber erzeugen den Wert, der das Symbol zeigt — und beide laufen in
Video-M (das Symbol erscheint ja), aber der Antrieb folgt nicht. `ndaction`
fährt vor jedem dieser Schreiber den Filter direkt (Debug-Shell ND-CLOSE /
ND-OPEN), in der Richtung des neuen Werts:

| Stelle | Änderung |
|---|---|
| `0x533BB848` | `bl 0x53583a00` → `bl Stub A` |
| `0x533BB82C` | `b 0x53583a18` → `b Stub B` |

Stub A (24 Byte ab `0x5301A754`): `push {r0-r3, lr}`; `cmp r2,#1`;
`bleq ND-CLOSE`; `blne ND-OPEN`; `pop {r0-r3, lr}`; `b 0x53583a00`.
Stub B (16 Byte ab `0x5301A76C`): `push {r0-r3, lr}`; `bl ND-CLOSE`;
`pop {r0-r3, lr}`; `b 0x53583a18`. (Die Stubs liegen im `osd`-Bereich, die
beiden Patches schließen sich aus; das Werkzeug bricht bei beiden ab.)

Wert-Konvention: 1 = ND drin (belegt: beim Drücken im ausgeschalteten
Zustand schreibt Weg B die 1, und dabei erscheint das Symbol „an").

Build: `tools/video-test.py build manual mlive7 dials bitrate ndaction`
(ohne `nd`, `ndprobe`, `osd`), Ausgabe `work/nd-action/FWDC239B.BIN`,
SHA-256 `0ab5d236291df7268881bf23008594d539b569c3e8a8601549cc595b762e577e`,
`fw-pack.py check` ok, alle Sprungziele im gebauten Bild mit Capstone geprüft.

**Ergebnis am Gerät (2026-10-16): auch `ndaction` ohne Wirkung** — in
Video-M rastet der Filter nicht ein. Das Thema ruht damit vorerst (drei
Anläufe: `nd`, `ndprobe`, `ndaction`).

Was damit ausgeschlossen ist: die Wert-Schreiber der Tastenaktion
(`0x53583a00`/`0x53583a18`) sind nicht der Ort, an dem der Antrieb hängt —
auch von dort aus fährt der ND im Video-M nicht (in Foto/P schon). Entweder
läuft die Taste in Video-M doch über einen anderen Schreiber (dann wäre das
Symbol nicht ihr Wert), oder der Antrieb selbst ist im Video-M gesperrt
(Freigabe `[obj+0x728] & 0x10`, FSS-Versand `0x64c40802` durch das
Bedienmodul).

Offene Spuren für einen späteren Anlauf:
- Familie `af8=0x40` / `af9=0x1e…0x35` (Werte-Push ans Objektiv, Masken
  0x100/0x1000/0x10000, Globals `0x55086d80`/`0x55086d90`/`0x55086d98`).
- AE-Übertragung `0x5348dd04` (nur bei `[settings+0x35] != 0`, sonst
  `0x53491fe0`) — möglicherweise der M-abhängige Weg.
- Antriebs-Freigabe: `[obj+0x728] & 0x10` (gesetzt `0x534401b4`, gelöscht
  `0x5344aaec`), ND-Anforderung `[obj+0x49]` → `0x53436738` (`[cmd+8] == 1`).
- Notnagel-Verdacht: `0x533BB820` (`02 00 00 0a` → `02 00 00 ea`) erzwingt den
  Umschalt-Zweig der Tastenaktion — nur sinnvoll, wenn der Pfad ausschließlich
  beim Tastendruck läuft.
