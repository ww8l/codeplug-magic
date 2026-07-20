# Contributing to WW8L Codeplug Magic

Thanks for your interest in improving WW8L Codeplug Magic! This document covers how to get a
development environment running and what's expected of a change before it's merged.

By contributing, you agree that your contributions are licensed under the project's
[GPL-3.0](LICENSE) license.

## Tech stack

- **Frontend:** React 19 + TypeScript + Vite + Tailwind CSS
- **Backend:** Rust with [Tauri v2](https://v2.tauri.app) and `sqlx` (SQLite)
- All application data lives in a single SQLite database in the OS app-data directory; schema
  changes are made with additive, numbered migrations under `src-tauri/migrations/`.

## Development setup

You need [Node.js](https://nodejs.org) 18+, a stable [Rust](https://rustup.rs) toolchain, and the
platform-specific [Tauri v2 prerequisites](https://v2.tauri.app/start/prerequisites/):

### Linux
```sh
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev \
  libayatana-appindicator3-dev librsvg2-dev libudev-dev \
  build-essential curl file
```
(`libudev-dev` is needed for USB serial-port enumeration.)

### macOS
```sh
xcode-select --install
```

### Windows
- Install the **Visual Studio C++ Build Tools** (the "Desktop development with C++" workload).
- The **WebView2 runtime** is preinstalled on Windows 11; on older Windows install it from Microsoft.

### Run the app
```sh
npm install
npm run tauri dev
```
The first Rust build is slow (it compiles all crates); subsequent runs are incremental.

## Project layout

```
src/                      React frontend (pages, components, api.ts bindings)
src-tauri/
  src/commands/           Tauri commands — one module per feature area
  src/radios/             One folder per radio driver, behind a shared trait
    driver.rs             The RadioDriver trait + its capability sub-traits
    registry.rs           The driver list; model → driver / format → exporter
  src/seed.rs             Built-in radio-model catalog (seeded at startup)
  migrations/             Numbered, additive SQLite migrations
  examples/               Headless CLI harnesses for hardware/RE work
scratchpad/               Local RE scratch (git-ignored, not contributor-facing)
```

## Before you open a pull request

Run the full local check suite and make sure it's clean:

```sh
# Rust: build, test, lint
cd src-tauri
cargo test
cargo clippy --all-targets

# Frontend: type-check + production build
cd ..
npm run build      # runs `tsc` then `vite build`
```

Guidelines:

- **Migrations are immutable once applied.** Never edit an existing migration file — its checksum
  is frozen and changing it will panic the app at startup. Add the next-numbered migration instead.
- **Keep changes focused.** One logical change per PR; describe what you changed and how you tested it.
- **Hardware-touching changes** (anything that reads from or writes to a radio) should note which
  radio was used to verify, since maintainers can't test every model. When in doubt, guard new
  write paths behind a backup + verify step, as the existing drivers do.
- Match the style and structure of the surrounding code.

## Reporting bugs and requesting radio support

Use the GitHub issue templates:

- **Bug report** — for something that doesn't work as expected.
- **New radio support** — to request programming or export support for a radio, including the
  details a driver needs (model, mode, connection type, and any existing CHIRP/CPS reference).

## Adding a new radio

Radios are **data plus a driver folder**. The app resolves everything about a radio from two
columns on its `radio_models` row — `driver_key` (which driver talks to it) and `programming_ui`
(which dialog programs it) — so adding one should never mean adding a branch to shared code.

> **The rule that keeps this true:** you should never need to touch `src-tauri/src/lib.rs`,
> `src/pages/Codeplugs.tsx`, or `src-tauri/src/commands/export.rs`. If you find yourself writing
> `if model == …` or `match driver_key` in any of them, the capability you need is missing from the
> trait — add it there instead.

Please open a **New radio support** issue before starting a driver, so we can check whether the
protocol is documented anywhere (CHIRP is often the fastest reference) and that nobody else is
already on it.

### Export-only support (no cable)

Often just one entry in `models()` in `src-tauri/src/seed.rs`: the radio's specs and limits, with
`driver_key: None` and `programming_ui: None`. Set `export_format` to `"chirp_csv"` for the shared
analog CSV, or to a registered exporter's format key. The seed table is upserted on
(manufacturer, model) at every startup, so no migration is needed and existing user profiles keep
working.

### Live USB driver checklist

1. **Pick a `driver_key`** — lowercase `manufacturer_model`, e.g. `tidradio_tdh3`. It is the folder
   name, the trait's `key()`, and a persisted foreign key in the user's database, so it is **never
   renamed once shipped**.

2. **Add the seed entry** with `driver_key: Some("…")` and `programming_ui: Some("generic")`.
   Set them together or not at all — a test rejects a half-migrated row, because both failure modes
   are silent at runtime (a bad `driver_key` only surfaces when someone plugs a radio in; a bad
   `programming_ui` just falls back to the generic dialog).

3. **Add a migration only if you need a new column.** A new radio normally doesn't. Existing
   migrations are immutable (see above).

4. **Write the driver** at `src-tauri/src/radios/<driver_key>/mod.rs`: a unit struct, a
   `pub(crate) static DRIVER`, and `impl RadioDriver` — `key`, `display_name`, `baud`, and
   `identify`. `identify` is a harmless handshake that reads no memory; it lives on the base trait
   because it is how a user proves the cable works before committing to anything.

   Then implement **only the capability sub-traits the radio actually supports**, overriding the
   matching `as_*` accessor for each (they all default to `None`):

   | Trait | For radios that… |
   | --- | --- |
   | `ImageProgrammer` | clone a whole memory image down and back up (UV-5R, TD-H3) |
   | `CodeplugProgrammer` | are programmed record-by-record from the database (AnyTone) |
   | `SettingsReader` / `SettingsWriter` | read / write non-channel settings. Separate traits on purpose: the UV-5R reads settings but has no standalone write — they ride out inside the image it uploads |
   | `ChannelWriter` | write channels without a full image |
   | `CallsignDbWriter` | carry an on-board DMR call-sign database |
   | `CodeplugExporter` | have their own file format (see below) |
   | `DriverDiagnostics` | need raw memory dumps during reverse-engineering |

   These accessors are the single source of truth: the frontend's capability flags are *derived*
   from them by `DriverCapabilities::of`, so a capability appears in the UI if and only if the
   trait is implemented. Never maintain a parallel list.

   Driver methods are **synchronous** — the async command layer resolves the database first and
   hands the driver already-resolved data plus a port name, inside `spawn_blocking`.

5. **Register it** — one `pub(crate) mod <driver_key>;` line in `src-tauri/src/radios/mod.rs`, and
   one line in the `DRIVERS` array in `src-tauri/src/radios/registry.rs`. It is a fixed-size array;
   **bump its length**.

6. **Settings**, if the radio has them: put `SettingsReader`/`SettingsWriter` in
   `radios/<driver_key>/settings.rs`, and describe the fields in the model's
   `non_channel_settings_schema` JSON. That schema drives both the generic settings form and the
   schema-driven write path — the fields do not need bespoke UI or a bespoke command.

7. **Frontend: usually nothing.** With `programming_ui: 'generic'`, the capability-driven dialog
   already offers identify, backup download, program, and verify. Only a radio needing genuinely
   bespoke controls earns its own dialog; if so, add it to `PROGRAM_DIALOGS` in
   `src/components/codeplugs/programDialogs.ts` **and** to the `UIS` array in the `seed.rs` test
   that pairs the two columns.

8. **Test what you can without hardware.** Protocol encode/decode is pure byte work — unit-test it
   directly (round-tripping an encoder against its decoder catches most bit-map mistakes). Put
   anything that needs a real radio in `src-tauri/examples/` as a headless harness rather than in
   the test suite, and say in your PR which radio you verified on.

### Adding a file export format

Implement `CodeplugExporter` on the driver and return your format key from `export_format()`. Set
the same string as the model's `export_format` in `seed.rs`; the export command looks the exporter
up **by that key**, not by driver, so an export-only model with no driver can use it too. Anything
no exporter claims — including `chirp_csv` and NULL — falls through to the shared CHIRP-compatible
analog CSV.
