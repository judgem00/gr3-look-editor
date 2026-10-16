# Web-Version des Look Editors (Plan)

**Stand 2026-10-12: Phasen 1–6 umgesetzt** im Repo
`gr3-look-editor`
(`core/`, `golden/`, `editor/static/bridge.js` + `worker.js`). Abweichung vom
Plan: GammaTuning ist nicht nachgebaut, sondern läuft im Original-ARM-Code in
einem kleinen Interpreter (`core/src/arm.rs`), bitgenau gegen Unicorn. Offen:
Repo öffentlich schalten und Pages aktivieren (Quelle „GitHub Actions“).

Der [Look Editor](../../../gr3x-fw-hack/tools/look-editor/README.md) als öffentliche, statische
Website. Die gesamte Logik läuft als WebAssembly im Browser; Firmware und DNG
zieht der Nutzer selbst hinein, nichts wird hochgeladen oder mitgeliefert.

## Entscheidungen

- **Hosting: GitHub Pages**, Deploy per GitHub Action (`wasm-pack` → `gh-pages`).
- **Kern in Rust → WebAssembly**, läuft in einem Web Worker. Pyodide nur als
  möglicher Prototyp (rawpy und Unicorn fehlen dort ohnehin).
- **Single-threaded.** GitHub Pages kann keine COOP/COEP-Header setzen, also
  kein `SharedArrayBuffer`. Reicht für 60 MB Image und eine Vorschau in
  reduzierter Auflösung; Threads später ggf. über `coi-serviceworker`.
- **Nur `fwdc239b` (GR III)**, erkannt per SHA-256 der Originaldatei. Alle
  Adressen gelten nur für diesen Build.

## Architektur

```
Browser
 ├─ UI: bestehendes static/app.js, fetch('/api/…') → worker.postMessage
 └─ Web Worker
     └─ core.wasm (Rust)
         ├─ fw     Version prüfen, decode, patch, repack, Prüfsummen  (grfw.py)
         ├─ looks  Matrizen, Tonkurven, Kanalkurven, Farbräder       (firmware.py, toneedit.py, chancurves.py, multiaxial.py)
         ├─ gamma  GammaTuning-Port                                  (fwgamma.py, heute Unicorn)
         └─ dng    GR-III-DNG, WB, eingebettetes JPEG, Makernote      (rawimage.py, heute rawpy + exiftool)
```

Die Worker-Nachrichten spiegeln die heutigen Flask-Endpunkte aus `server.py`.
Export: `FWDC239B.BIN` und `*.looks.json` als Download-Blob.

## Risiken

- **GammaTuning:** Nachbau statt Emulation, bitgenau gegen Unicorn geprüft
  (alle 9⁴ Reglerkombinationen × zufällig bearbeitete Basiskurven).
  Ausweichweg: Unicorn als WebAssembly.
- **DNG-Leser:** eigener, nur für GR-III-DNGs; Ausweichweg libraw-wasm.

## Schutz der Nutzer

- Nur die bekannte Firmware annehmen (SHA-256).
- Selbsttest beim Laden: unverändert neu gepackt = byteidentisch.
- Vor dem Download: erzeugte Datei wieder entpacken, mit dem gepatchten Image
  vergleichen, alle `FwDecoder`-Prüfungen nachrechnen.
- Deutlicher Hinweis auf das Risiko und den Recovery-Weg.

## Rechtliches / Auftritt

- Firmware nie im Repo oder auf der Seite; auf Ricohs Download-Seite verlinken.
- Name ohne Anschein von Offizialität („Inoffizieller Look-Editor für die
  GR III"), kein Ricoh-Logo, Hinweis auf fehlende Verbindung zu Ricoh.
- Impressum/Datenschutz prüfen; die Seite selbst verarbeitet keine Daten,
  nur GitHub speichert Zugriffs-Logs.
- Vorbild für das Muster: Browser-Port von Halo CE (Disc-Image vom Nutzer,
  Build per SHA-256 identifiziert, keine Spieldaten ausgeliefert).

## Phasen

1. Golden-Daten aus Python: Pack-Ergebnis, Matrizen, GammaTuning-Vektoren,
   Vorschau-Referenzbilder.
2. Rust-Kern `fw` + `looks`: byteidentische `FWDC239B.BIN` zum heutigen
   Export, erst als CLI getestet.
3. GammaTuning-Port mit Differenztest gegen Unicorn.
   → Ab hier nutzbar: Preset-JSON + Firmware rein, Update-Datei raus.
4. DNG-Leser und Vorschau-Pipeline, gegen Referenzbilder.
5. Worker + UI, Drag & Drop, Download.
6. GitHub Action, Pages, Hinweistexte.

## Repo

Eigenes, schlankes Repo nur für die Web-Version (nicht `hhornbacher/gr3x-fw-hack`),
`gr3-look-editor`,
vorerst privat (öffentlich schalten, sobald Pages gebraucht wird), MIT. Enthält Rust-Kern, UI, Golden-Daten-Skripte
und die Pages-Action; die Analysedokus liegen jetzt hier unter `docs/firmware/`.
Keine Firmware, keine DNGs (`.gitignore`: `*.bin`, `*.BIN`, `*.dng`, `*.DNG`).
