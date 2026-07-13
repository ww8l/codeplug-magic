---
name: New radio support
about: Request support for a radio (CSV export or direct USB programming)
title: "[Radio] "
labels: new-radio
assignees: ''
---

## Radio

- Manufacturer:
- Model:
- Modes: (analog FM, DMR, D-STAR, YSF, etc.)
- Bands: (VHF / UHF / 1.25 m / 900 MHz / HF)
- Approximate memory-channel count:
- Does it support zones / scan lists / banks?

## What kind of support?

- [ ] CSV export only (generate a file to import with the vendor tool)
- [ ] Direct USB programming (read / write over a cable)

## Connection

- Cable / interface: (e.g. Kenwood K1 2-pin, USB-C, proprietary)
- Does the radio show up as a serial port when plugged in? If so, what does it enumerate as?

## Existing references

Anything that documents the radio's memory format helps a lot:

- CHIRP driver (link, if one exists):
- Vendor CPS / other programming software:
- Any published memory maps, `.img`/codeplug dumps, or protocol notes:

## Do you own this radio?

Driver work needs hardware verification. Let us know if you have the radio and can test builds,
or capture read/write traces.
