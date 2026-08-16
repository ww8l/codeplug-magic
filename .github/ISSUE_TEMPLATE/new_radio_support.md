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

## What programming software do you have for it?

RT Systems, the manufacturer's own CPS, or none. Any of them can be used as a *measuring
instrument* while the memory map is worked out — the app itself never depends on one, and you're
never asked to own one to use it. Knowing which is available changes how the settings get found.

---

<details>
<summary>How support actually gets added (maintainer checklist)</summary>

Each step has a gate that has to hold before the next one starts. Deviating a little is fine;
skipping a gate is how radios get bricked.

- [ ] **1. Triage** — search for a published format first (CHIRP, RE repos, the manual). Decide
      whether this is a clone of a radio already supported, a new card format, or a new cable
      protocol. *Gate: a written plan naming the sources and the programming medium.*
- [ ] **2. Anchor on a radio-authored file** — two saves with one change between them, plus a
      no-change save for the noise floor. *Gate: the published map decodes against a real file.*
- [ ] **3. Container + memory encoder**, no hardware. *Gate: re-encoding the radio's own memories
      out of its own save is byte-identical.*
- [ ] **4. Seed the model and wire the path** — bands (including where the receiver has gaps),
      capacity, banks/zones/scan lists. *Gate: the app's own export pipeline, not just unit tests.*
- [ ] **5. Measure the settings** with whatever instrument is available, validated first.
      *Gate: every address graded measured / inferred / unverified.*
- [ ] **6. Generate the field table and form schema from one sheet**, and wire both ends —
      including the encoder into the export. *Gate: an export carries memories **and** settings.*
- [ ] **7. Hardware ladder** — identity write → one-byte write → full codeplug → band probe →
      settings spot check, in order, stopping at the first failure.
- [ ] **8. Land it** — verified in dev, committed, pushed.

</details>
