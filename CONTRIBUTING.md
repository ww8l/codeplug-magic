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

> A structured "add a new radio driver" checklist will be added here once the driver-modularity
> refactor lands. Until then, adding **CSV-export** support for a radio is often just a new seed
> entry in `src-tauri/src/seed.rs`; live USB programming is more involved — please open a
> "New radio support" issue to discuss before starting.
