# Issue #NN — <Manufacturer> <Model> support

Copy to `scratchpad/<driver_key>/PLAN.md` and fill in. Written **before** any
code, and kept current — a resumed session reads this first.

## Context

What the issue asks for, and what the research turned up. Published sources are
searched before any reverse-engineering.

| Source | What it gives | Verified against a real file? |
|---|---|---|
| CHIRP `chirp/drivers/<x>.py` | | |
| `github.com/…` | | |
| The radio's manual | menu order, option lists, defaults | |

State plainly which of these has been **checked against a file this radio
actually wrote**, and which is still taken on trust. That distinction decides
how much of the plan below is real.

**Family:** clone of <radio we already support> / new card format / new cable
protocol. If it is a clone, say exactly what carries over and what must still be
re-measured.

**What the user has:** radio / cable / microSD card / programming software
(RT Systems, OEM CPS, none) / availability for hardware steps.

## Shape of the work

`driver_key = "<manufacturer>_<model>"`, `export_format = "<key>"`,
`programming_ui = "generic"`. Which capability sub-traits the driver claims, and
why the others are absent. Branch `issue-NN-<slug>`.

⚠ Note here anything the two candidate images do *not* share — length, header,
revision stamps — so no address is assumed to transfer between them.

---

## Phase 1 — Anchor on a real file (hardware, ~15 min)

Nothing is written until the radio's own output is in hand. Two saves with one
change between them, plus a no-change save for the noise floor. Verify the
published offsets decode against it, and have the user confirm values on the
radio's screen.

Deliverable: `scratchpad/<key>/FINDINGS.md`.

## Phase 2 — Container + memory encoder (pure Rust, no hardware)

`src-tauri/src/radios/<driver_key>/`:

- container module — header preserved byte-for-byte, body handed out for
  patching, model/revision guard that refuses a foreign file by name.
- memory module — channels at the mapped offsets; channel lists → the radio's
  banks/groups/zones; enums from published tables cross-checked with the manual.
- ⚠ Checksum: state what is believed and **how it will be tested** in Phase 4
  step 2. An inherited "no checksum apparent" is not a finding.

Decisive test: re-encode the radio's own memories out of its own save,
byte-identical. Plus an encode→decode round-trip over the field matrix.

## Phase 3 — Seed the model and wire the path

- `seed.rs` — `tx_bands` from spec; `rx_bands` modelling the receiver's real
  **gaps**; channel count, name length, and which of banks/zones/scan lists are
  genuine. Settings schema starts empty until Phase 4.
- `radios/mod.rs` + `registry.rs` (bump the array length), the card-finder arm
  or programming path, and the media-write instructions in the frontend.
- Write a **new** timestamped file; never patch the user's save in place.
- Frontend otherwise: nothing, with `programming_ui: 'generic'`.

## Phase 4 — Settings

1. Read the manual first and plan the passes from it.
2. Stand up whichever instrument is available; validate it before trusting it.
3. Measure by diffing tool-vs-tool, one unique `old -> new` pair per control.
4. Generate the field table **and** the form schema from one sheet.
5. Wire the read command, its binding, the form entry, and `apply_settings` into
   the export — then prove the export carries settings.

Deliverable: `scratchpad/<key>/MEASURED.md`, every row graded.

## Phase 5 — Hardware verification

Identity write → one-byte write → full codeplug (check every group) → band
probe at the coverage edges → settings spot check. In order, stopping at the
first failure, each asserting its property before the file is handed over.

## Files

**New** — …

**Edited** — …

**Untouched, by rule** — `src-tauri/src/lib.rs` (beyond command registration),
`src/pages/Codeplugs.tsx`, `src-tauri/src/commands/export.rs`. Needing a branch
in any of them means a capability is missing from the trait.

## Verification

- `npm run ci` (the pre-push hook — there is no macOS CI leg, so this is it).
- No `cargo fmt`.
- Unit: byte-identical re-encode; round-trip; table↔schema agreement; an
  `rx_bands`-excludes-what-the-radio-cannot-tune test carrying the reason.
- App: run dev, build a codeplug on the new model, write it, confirm the report.
- Hardware: Phase 5, on the real radio.
- Land: verified in dev → commit → push. No PR until a release batches branches.

## Risks

| Risk | Mitigation |
|---|---|
| | |
