# Plan: eigene Farbprofile für die GR III

Arbeitsdokument. Ziel sind **echte neue Farbprofile** — nicht andere
Einstellungen der vorhandenen Regler, sondern ausgetauschte Tabellen in der
Bildpipeline.

Stand: 26.08.2026. Firmware `fwdc239b` (GR III, Build 2025-09-25), entpackt,
im Emulator lauffähig, Tuning-Daten teilweise kartiert.

## 1. Was bewiesen ist

**Die Pipeline ist tabellenbasiert.** Der ISP hat eine LUT-Hardwareeinheit mit
vier Kanälen (`LUTCTL_A`…`_D` mit Vorberechnungsregistern, angesteuert von
`CMultipleImageProcessLutDriver`). Farbtransformation läuft über Tabellen, die
in Hardware geladen werden — nicht über fest verdrahtete Formeln.

**Die Tabellen liegen als Daten im Image.** Gefunden mit
[`tools/find-luts.py`](../../../gr3x-fw-hack/tools/find-luts.py):

| Datei-Offset | Inhalt |
|---:|---|
| 14.558.190 | 14-Bit-Tonkurve, 16.386 Einträge, gekrümmt (hebt Schatten an) |
| ~14,55–14,60 MB | drei Tabellen à 2048 Einträge, voller 16-Bit-Bereich |
| 15,81–15,92 MB | Kurvenbank: 50+ Kurven à 1024 Stützstellen |
| 14.498.368 | 3×3-Farbmatrix, Festkomma-Skala 512, entsättigend |
| 14.507.656 / 14.507.764 | 3×3-**Identitätsmatrizen**, Skala 8192 |

Die Identitätsmatrizen sind aufschlussreich: die Struktur ist vorhanden und
wird je nach Kontext mit Werten gefüllt. Wo ein Look Farbe verschiebt, steht
dort eine andere Matrix.

**Die Ladeadresse ist bestimmt und verifiziert.** Bias `0x52FEF790`, also
`RAM-Adresse = Datei-Offset + 0x52FEF790`. Zwei unabhängige Codestellen
bestätigen ihn, und er passt zur Speicherkarte aus dem `PARTPARA`-Header
(Datei-Offset `0x10870` → `0x53000000`, die 64-MB-Region). Werkzeug:
[`tools/find-load-address.py`](../../../gr3x-fw-hack/tools/find-load-address.py).

Damit sind Querverweise auflösbar — die Voraussetzung dafür, überhaupt
herauszufinden, welche Tabelle wofür benutzt wird.

**Es gibt eine zentrale Tuning-Registry.** Bei Datei-Offset 17.454.552
(RAM `0x54094d68`): **30.628 Zeiger auf 1.095 verschiedene Ziele**. Darin
unter anderem eine Kurvenfamilie mit sieben systematisch abgestuften Kurven
(jede rund 60 % der vorherigen Steilheit) — das Muster eines Parameters mit
Bereich −3…+3.

## 2. Was offen ist

1. **Die Farbdaten sind erst teilweise gefunden.** `m_MultiAxialTuning`
   (Hue/Sättigung pro Farbachse) ist für einen Farblook zentral und noch nicht
   lokalisiert. In der bekannten Registry stehen nur drei 3×3-Matrizen; die
   Weißabgleich-CCMs müssen woanders liegen.
2. **Die Zuordnung Look → Tabelle ist nicht bewiesen.** Wir haben Tabellen und
   die Mittel, Referenzen aufzulösen — aber noch keine Kette von „Positive
   Film" zu konkreten Daten.
3. **`CalcTable` / `DigitalFilterGammaCoordinator`**: Finale LUTs werden
   offenbar zur Laufzeit aus Basistabellen berechnet. Dann ist die Basis das
   Ziel eines Patches, nicht das Endergebnis. `ToneTableSecureKeeper.cpp`
   sollte auf eine Integritätsprüfung hin angesehen werden.
4. **Der Flash-Weg.** Siehe unten — der kritische Pfad.

## 3. Der kritische Pfad

Da nur echte Profile in Frage kommen, führt kein Weg an modifizierter Firmware
vorbei. Alles andere ist Vorarbeit, die wertlos bleibt, wenn dieser Punkt nicht
gelöst ist:

> **Kann eine geänderte Firmware auf die Kamera?**

Bekannte Hinweise im Image: `No valid EB signature found`, `PRSA`,
`CHECKSUM NG`. Ob diese den Update-Pfad tatsächlich absichern, ist ungeklärt.

Deshalb steht das jetzt an erster Stelle — vor jeder weiteren Analyse.

## 4. Phasen

### Phase 1 — Flash-Weg klären (kritisch, zuerst) — ABGESCHLOSSEN

Ergebnis in [firmware-update-security.md](../../../gr3x-fw-hack/docs/firmware-update-security.md).

- **Signaturprüfung: keine.** Nur eine additive Prüfsumme gegen ein einzelnes
  Header-Byte, im Maschinencode verifiziert. Keine Kryptobibliothek im Image.
  Reproduziert in [`tools/fw-checksum.py`](../../../gr3x-fw-hack/tools/fw-checksum.py).
- **Recovery: Anzeichen vorhanden, nicht bestätigt.** SD-Boot-Modus des
  M0-Supervisors (eigene Partition, überlebt einen Brick der Hauptfirmware),
  Service-Dump/Restore (`FlashMemoryFactory`), Factory-Full-Update. Ob SD_BOOT
  ein voll gebricktes Gerät rettet, ist statisch nicht beweisbar.
- **Das Update flasht in-place** in den eMMC — Unterbrechung mitten im
  Schreiben ist der Brick-Fall. Modifizierte Firmware muss die Projektnummer
  erhalten (`project number is different!`).

> **Gate 1: vorsichtig positiv.** Technisch steht modifizierter Firmware
> nichts im Weg. Bevor aber irgendetwas geflasht wird, muss der Recovery-Weg
> **an der Hardware** bestätigt sein: SD_BOOT-Trigger recherchieren, per
> Service-Dump ein vollständiges Backup ziehen, Rückweg praktisch verifizieren.

### Phase 2 — Farbdaten kartieren — WEITGEHEND ABGESCHLOSSEN

Ergebnis in [colour-pipeline-data.md](colour-pipeline-data.md): 29 Farbmatrizen
(int16, Skala 8192) in einem Block bei 14.490.784, ein Array aus 60 neutralen
Matrizen bei 14.498.404, Tonkurven und Parameterstufen. Offen bleibt
`MultiAxialTuning` und die Zuordnung Look → Daten.

### Phase 2 (Rest) — MultiAxial und Zuordnung

- `MultiAxialTuning` lokalisieren: die Struktur, die Hue und Sättigung pro
  Farbachse verschiebt
- Weitere Registries suchen — die bekannte ist vermutlich nicht die einzige
- Die Weißabgleich-CCMs finden (3×3-Matrizen pro Lichtart)

**Ergebnis:** Der vollständige Satz an Stellschrauben für ein Farbprofil.
**Risiko:** keins.

### Phase 3 — Zuordnung Look → Tabelle beweisen

Mit dem bekannten Bias die Referenzen verfolgen: Welcher Code liest welche
Tabelle, und unter welcher Bedingung? Ziel ist eine belastbare Kette von einem
Bildstil zu seinen Daten — nicht Interpretation, sondern Nachweis.

Ergänzend, falls die Codeanalyse zäh wird: **Firmware-Versionen vergleichen.**
Ricoh hat Bildstile in Updates ergänzt; ein Diff zeigt, welche Daten mit einem
neuen Stil dazukamen. Auch ein Vergleich mit der GR III Monochrome (die keine
Farbstile hat) wäre aufschlussreich.

### Phase 4 — Eigenes Profil bauen und testen

Erst im Emulator so weit wie möglich verifizieren, dann — nur bei positivem
Gate 1 — auf der Kamera. Klein anfangen: eine bekannte Tabelle minimal ändern
und prüfen, ob sich das erwartete Verhalten zeigt.

## 5. Risiken

| Risiko | Auswirkung | Umgang |
|--------|-----------|--------|
| Firmware signiert, kein Bypass | Projektziel unerreichbar | Phase 1 zuerst, bevor Aufwand in Analyse fließt |
| Kein Recovery-Pfad | Bricking ist endgültig | ohne Recovery nicht flashen |
| Tabellen werden zur Laufzeit berechnet | Patch der Basis wirkt anders als erwartet | `CalcTable` verstehen, bevor gepatcht wird |
| Integritätsprüfung der Tuning-Daten | Patch wird abgelehnt | `ToneTableSecureKeeper` prüfen |
| Teile der Pipeline im CEVA-DSP | dort kein Reverse Engineering möglich | hoffen, dass die Tabellen ARM-seitig liegen — bisher sieht es danach aus |

Die Kamera kann bei einem misslungenen Flash unbrauchbar werden, und die
Garantie ist dann ohnehin verloren. Das ist der Preis dieses Ziels; die
Parameter-Variante hätte ihn nicht.

## 6. Werkzeuge

| Werkzeug | Zweck |
|---|---|
| [`tools/find-luts.py`](../../../gr3x-fw-hack/tools/find-luts.py) | findet Tabellen über monotone Wertfolgen |
| [`tools/find-load-address.py`](../../../gr3x-fw-hack/tools/find-load-address.py) | Ladeadresse über `movw`/`movt`-Muster |
| [`tools/romfs-extract.py`](../../../gr3x-fw-hack/tools/romfs-extract.py) | romfs ohne `mount` entpacken |
| [`emulator/`](../../../gr3x-fw-hack/emulator/README.md) | führt die Linux-Seite der Firmware aus |

Der Emulator trägt zur Farbanalyse wenig bei: Er führt die Linux-Seite aus,
nicht die Bildpipeline. Für Farbprofile ist die statische Analyse der Weg.

## 7. Nächster Schritt

Phase 1. Solange nicht geklärt ist, ob eigene Firmware auf die Kamera kommt,
ist jede weitere Analyse eine Wette.
