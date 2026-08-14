# WW8L Codeplug Magic

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)

A cross-platform desktop app for building and managing amateur-radio **codeplugs** — the
channel, zone, scan-list, and contact programming that goes into a handheld or mobile radio.

Instead of juggling a separate vendor programming tool (CPS) per radio, you maintain **one
master database** of repeaters and channels, then generate or directly program a codeplug for
whichever radio you own. Channels can be imported from RepeaterBook, geocoded from a city name,
organized into zones and scan lists, and pushed straight to the radio over USB or its microSD
card — or exported as CSV for tools that expect it.

> **Status:** working prototype, actively developed. Programming paths for the radios below have
> been verified on real hardware, but this is early software — always keep a backup of your
> radio's existing codeplug before writing to it.

## Supported radios

| Radio | Modes | Bands | Programming | Notes |
|-------|-------|-------|-------------|-------|
| **Baofeng UV-5R** | Analog FM | VHF / UHF | Direct USB — read, write, settings | 128 channels, flat memory |
| **TIDRADIO TD-H3** | Analog FM/NFM/AM | VHF / UHF / 1.25 m | Direct USB — read, write, settings | 200 channels, tri-band |
| **AnyTone AT-D890UV** | DMR + Analog | VHF / UHF | Direct USB — channels, zones, scan lists, settings, call-sign DB | 4000 channels; full DMR: zones, talkgroups, 308k-entry caller-ID database |
| **Yaesu FT5D** | C4FM (System Fusion) + Analog | VHF / UHF TX, wideband RX | microSD — patches the radio's own backup file | 900 channels in 24 banks; channels, banks, and menu settings |
| **Icom ID-52** | D-STAR + Analog | VHF / UHF TX, 108–174 / 225–479 MHz RX | microSD — patches the radio's own `.icf` file | 1000 memories in 100 groups; memories and menu settings restore in one operation |

Direct USB programming reads the radio's current image, applies your changes, backs up the
original, writes, and can verify the result byte-for-byte.

Cable-free radios are programmed through their own microSD card instead: the app patches the
file the radio itself wrote — the FT5D's `BACKUP.dat`, the ID-52's `.icf` — so the radio restores
it from its own menu with no vendor software involved. Because those files carry the radio's
settings as well as its memories, the codeplug and the radio profile travel together.

Channels the radio can hear but not transmit on (GMRS, marine, NOAA, air band, 220) are
programmed **receive-only** rather than dropped, and frequencies outside the receiver's real
coverage are excluded with a reason rather than silently written to an empty slot.

## Features

- **Master channel database** — one library of repeaters/channels reused across every codeplug.
- **Import** from RepeaterBook, CHIRP-style CSV, or manual entry with automatic lat/lon geocoding
  from a city name.
- **Zones, scan lists, and talkgroups** with per-channel scan-list assignment.
- **DMR contacts** — a local mirror of the [radioid.net](https://radioid.net) DMR-ID → callsign
  database, with prioritized export and (on the D890UV) direct caller-ID database programming.
- **Direct radio programming** over USB with backup, write, and byte-level verify.
- **microSD programming** for radios that take a card instead of a cable (Yaesu FT5D, Icom ID-52),
  patching the radio's own backup/settings file so its menus and memories come back together.
- **Radio profiles** — every radio's menu settings are read out of the radio (or its card file),
  edited in the app, and written back alongside the channels.
- **CSV / JSON export** for radios without a direct driver.
- **Whole-database backup & restore** to a single portable file.

## Download

Prebuilt installers for macOS, Windows, and Linux are published on the
[Releases page](https://github.com/ww8l/codeplug-magic/releases).

Builds are currently **unsigned**, so the OS will warn on first launch:

- **macOS** — right-click the app → **Open**, then confirm (Gatekeeper blocks a normal
  double-click on unsigned apps).
- **Windows** — SmartScreen → **More info** → **Run anyway**.
- **Linux** — mark the `.AppImage` executable (`chmod +x`) or install the `.deb`.

## Build from source

**Prerequisites (all platforms):** [Node.js](https://nodejs.org) 18+, [Rust](https://rustup.rs)
(stable), and the [Tauri v2 prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS:

- **Linux:** `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libudev-dev` (plus a C toolchain)
- **macOS:** Xcode Command Line Tools (`xcode-select --install`)
- **Windows:** Visual Studio C++ Build Tools and the WebView2 runtime (preinstalled on Windows 11)

```sh
git clone https://github.com/ww8l/codeplug-magic.git
cd codeplug-magic
npm install

# Run the app in development
npm run tauri dev

# Build a release bundle for your platform
npm run tauri build
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full development workflow, test/lint expectations,
and platform-specific notes.

## License

WW8L Codeplug Magic is licensed under the **GNU General Public License v3.0** — see [LICENSE](LICENSE).

Portions of the radio protocol logic are derived from [CHIRP](https://chirpmyradio.com), which is
also GPLv3: struct layouts, tone tables, and encode/decode routines for the Baofeng and TIDRADIO
drivers; the FT5D settings map (from `ft1d.py`); and the Icom `.icf` container format (from
`icf.py`).
