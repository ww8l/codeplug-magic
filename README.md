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
| **Kenwood TH-D72** | APRS + Analog | 2 m / 70 cm TX, 118–174 / 320–524 MHz RX | Direct USB — read, write, settings | 1000 memories; 113 menu settings over the radio's own `MU` command |
| **Kenwood TH-D75** | D-STAR + APRS + Analog | VHF / 1.25 m / UHF TX, 0.1–524 MHz RX | microSD — patches the radio's own `.d75` file | 1000 memories in 30 groups; memories and menu settings, including the APRS setup |
| **Binteradio BT-9000** | Analog FM/NFM | 18–64 / 136–174 / 200–260 / 400–520 MHz TX, 18–999 MHz RX | Direct USB — read, write, settings | 960 channels in 15 fixed zones; 42 menu settings. Also sold as the Radtel RT-950 Pro, Bajeton BJ-9000 and Tenway TP-900 Pro — the radio reports itself as `RT-950` |

Direct USB programming reads the radio's current image, applies your changes, backs up the
original, writes, and can verify the result byte-for-byte.

Cable-free radios are programmed through their own microSD card instead: the app patches the file
the radio itself wrote — the FT5D's `BACKUP.dat`, the ID-52's `.icf`, the TH-D75's `.d75` — so the
radio restores it from its own menu with no vendor software involved. Because those files carry
the radio's settings as well as its memories, the codeplug and the radio profile travel together.

Channels the radio can hear but not transmit on (GMRS, marine, NOAA, air band, 220) are
programmed **receive-only** rather than dropped — where the radio has a per-channel transmit
inhibit, the app sets it — and frequencies outside the receiver's real coverage are excluded with
a reason rather than silently written to an empty slot.

Band limits are measured on the radio, not copied from its manual. The BT-9000's 220 MHz
transmit capability, for instance, appears in no published source for it and was found by
programming a channel there and keying up.

## Future development

Every radio below has an open issue tracking its support. They are added one at a time using the
same process each radio in the table above went through: research the published format first,
measure anything unpublished against real hardware, build channels **and** the radio's menu
settings together, then verify on the actual radio before shipping.

**Planned**

| Radio | Modes | Issue |
|-------|-------|-------|
| **AnyTone AT-D578UV** | DMR + Analog mobile | [#47](https://github.com/ww8l/codeplug-magic/issues/47) |
| **AnyTone AT-D868UV** | DMR + Analog handheld | [#51](https://github.com/ww8l/codeplug-magic/issues/51) |
| **Icom ID-51** | D-STAR + Analog handheld | [#50](https://github.com/ww8l/codeplug-magic/issues/50) |
| **Kenwood TM-D710** | APRS + Analog mobile | [#113](https://github.com/ww8l/codeplug-magic/issues/113) |
| **Icom ID-5100** | D-STAR + Analog mobile | [#49](https://github.com/ww8l/codeplug-magic/issues/49) |
| **Icom IC-9100** | HF / VHF / UHF base | [#45](https://github.com/ww8l/codeplug-magic/issues/45) |
| **Icom IC-7610** | HF / 6 m SDR base | [#46](https://github.com/ww8l/codeplug-magic/issues/46) |
| **Quansheng UV-K5** | Analog handheld | [#44](https://github.com/ww8l/codeplug-magic/issues/44) |
| **TYT MD-380** | DMR + Analog handheld | [#42](https://github.com/ww8l/codeplug-magic/issues/42) |
| **Yaesu FT-891** | HF / 6 m mobile | [#53](https://github.com/ww8l/codeplug-magic/issues/53) |
| **Yaesu FTM-300** | C4FM + Analog mobile | [#48](https://github.com/ww8l/codeplug-magic/issues/48) |
| **Yaesu FTM-400** | C4FM + Analog mobile | [#52](https://github.com/ww8l/codeplug-magic/issues/52) |

Adding a radio takes hardware to verify against, so the order these land in follows what's on the
bench. If you own one of these and want to help measure or test it, say so on its issue — see
[CONTRIBUTING.md](CONTRIBUTING.md).

## Features

- **Master channel database** — one library of repeaters/channels reused across every codeplug.
- **Import** from RepeaterBook, CHIRP-style CSV, or manual entry with automatic lat/lon geocoding
  from a city name.
- **Zones, scan lists, and talkgroups** with per-channel scan-list assignment.
- **DMR contacts** — a local mirror of the [radioid.net](https://radioid.net) DMR-ID → callsign
  database, with prioritized export and (on the D890UV) direct caller-ID database programming.
- **Direct radio programming** over USB with backup, write, and byte-level verify.
- **microSD programming** for radios that take a card instead of a cable (Yaesu FT5D, Icom ID-52,
  Kenwood TH-D75), patching the radio's own backup/settings file so its menus and memories come
  back together.
- **D-STAR call signs** — per-channel URCALL / RPT1 / RPT2, so a D-STAR repeater channel carries
  its own routing; left blank, it falls back to deriving them from the repeater's call sign.
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

### Upstream projects this work builds on

Every borrowing is also cited at its use site in the source. This section is the summary.

**[CHIRP](https://chirpmyradio.com)** — GPLv3, compatible. Struct layouts, tone tables, and
encode/decode routines for the Baofeng and TIDRADIO drivers; the FT5D settings map (from
`ft1d.py`); the Icom `.icf` container format (from `icf.py`); and the TH-D75 channel memory layout
(from `thd74.py`).

**[qdmr](https://github.com/hmatuschek/qdmr)** — GPLv3, © Hannes Matuschek DM3MAT; compatible.
The AnyTone AT-D890UV driver takes its call-sign database per-field length limits from qdmr's
`d868uv_callsigndb` encoder and its contact-index semantics, both in
`radios/anytone_atd890uv/callsign_db.rs`, plus the scan-list value table in
`radios/anytone_atd890uv/mod.rs`.

**[dmr-tools/codeplugs](https://github.com/dmr-tools/codeplugs)** — **no LICENSE file**, therefore
all-rights-reserved by default. Its `d890uv v1.05` descriptor is the source of the 209-entry
AnyTone settings table (`radios/anytone_atd890uv/anytone_settings_table.rs`, mechanically generated
from it), and of the flash region map, channel-record layout, CTCSS tone-table ordering, scan-list
record and call-sign DB layout. Byte offsets are facts and carry no copyright, and every offset
here was validated against a golden dump from a real radio — but the generated table is a
mechanical transform of one upstream file and is the concentrated exposure. Asking upstream to
add an explicit licence is an open action, not something already done.

**[reald/anytone-flash-tools](https://github.com/reald/anytone-flash-tools)** — bare
**GPL-2.0-only**, with no "or any later version" grant, which cannot be combined into a GPLv3 work.
**No code, table, or data from it is present here, and none may be added.** What was taken is a
list of protocol facts from its `at-d878uv_protocol.md` — send `PROGRAM`, expect `QX\x06`, the
`R`/`W` frame layout, `END` to finish — and how a device behaves on the wire is not copyrightable
subject matter. It is credited because that is where those facts were learned, not because anything
of its was copied.
