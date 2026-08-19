# Security policy

WW8L Codeplug Magic writes to the memory of physical radios over USB, serial cables and
microSD cards. A bug in that path can leave a radio unusable, so security reports are
taken seriously here even though this is a small hobby project.

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

Use GitHub's private vulnerability reporting: the **Security** tab → **Report a
vulnerability**. That opens a private advisory visible only to you and the maintainer.

Expect an acknowledgement within about a week. This is a one-person project with no paid
support, so please read that as an honest estimate rather than an SLA. There is no bug
bounty.

When you report, please **do not attach a codeplug or `.img` dump from a radio you have
programmed** — it contains your call sign, DMR ID, APRS position and contact list. See the
note in the issue templates. A factory-reset dump, or a description of the bytes involved,
is enough.

## What is in scope

- The application itself: the Tauri backend, the React frontend, and the IPC boundary
  between them.
- The radio drivers under `src-tauri/src/radios/` — in particular anything that could
  cause a write to land at the wrong address, or a malformed file to be accepted as a
  valid image.
- Parsing of untrusted input: imported CSV/JSON, restored database backups, and codeplug
  or image files loaded from disk.
- The local SQLite database and the backup/restore paths.

## What is out of scope

- The radios' own firmware, and vendor programming software. If a radio misbehaves when
  correctly programmed, that is a matter for its manufacturer.
- Third-party services the app can import from, such as RepeaterBook and radioid.net.
- The inherent fact that programming a radio requires write access to it. That is the
  purpose of the application, not a vulnerability.
- Physical access to an unlocked machine. The database is not encrypted at rest, and is
  not intended to be.

## What the app stores, and where

Everything is local. There is no server, no account and no telemetry.

- The database — your channels, codeplugs, radio profiles and any downloaded DMR user
  list — lives in the platform application-data directory.
- Radio backups written before a programming operation live in `radio-backups/` beside it.
  These are full images of your radio and carry the personal data described above.

Two features reach the network, both only when you ask: importing repeater data, and
geocoding a city name to fill in coordinates.

## Supported versions

Only the most recent release is supported. Fixes land on `main` and go out in the next
release rather than being backported.
