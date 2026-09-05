---
name: new-radio
description: The standard process for adding support for a new radio to CodePlug Magic — triage and research, anchoring on a radio-authored file, the memory encoder, measuring settings with whatever programmer is on hand, and the hardware verification ladder. Load this when starting or continuing any radio-support issue (they carry the `new-radio` label), when asked to "add support for" a radio by name or issue number, or before substantial work under `src-tauri/src/radios/`.
---

# Adding a radio

This is how the last three radios were built. It is a shape to follow, not a
script to obey — deviating a little where a radio calls for it is expected and
fine. What is *not* optional is the gate at the end of each step: those exist
because skipping one has already cost hardware time, and in one case a factory
reset.

Two things to establish before anything else, both of which are questions for
the user rather than something to figure out:

- **What do they actually have?** The radio, a cable, a microSD card, and which
  programming software — RT Systems, the OEM CPS, or none. This decides step 5
  and sometimes step 2.
- **Are they available to run hardware steps?** Steps 2 and 7 need someone at
  the radio. Everything else is desk work and can proceed without them.

`CONTRIBUTING.md` ("Adding a new radio") holds the code-wiring checklist —
`driver_key` naming, the seed row, the capability sub-traits, the registry
entry. This skill is the *investigation* half and does not repeat it.

---

## 1. Triage — research before reverse-engineering

Search for a published format **first**. Reverse-engineering is the fallback,
not the opening move, and this has paid off on every radio where it was tried.

| Source | What it tends to give |
|---|---|
| CHIRP (`chirp/drivers/*.py`) | Whole channel memory layouts, sometimes settings, sometimes the clone protocol |
| GitHub RE repos for the model or its siblings | Config-file structure, command tables, enum tables |
| **The radio's own manual** | Menu order, option lists, defaults — and it is a *published source*, not a fallback |

### ★ First enumerate what the RADIO has, not what a source describes

Do this before reading any source in depth, and write it down. A published table
covers what its author needed; the gap between that and the radio is invisible
unless you have the radio's own list to hold it against.

1. **What is this radio FOR?** Read the model's feature list — the manual's
   first pages, the manufacturer's product page. A built-in TNC, a GPS, D-STAR,
   a second receiver, cross-band repeat: each is a whole family of settings.
   Manufacturer naming often carries it (Kenwood's `D` in TM-**D**710 and
   TH-**D**75 means the data/APRS half; the TM-V71 is the same radio without it).
2. **Enumerate the complete menu map** from the manual — every group, and how
   many menus are in each. This is the denominator for everything after it.
3. **For each group, name the transport that reaches it.** A group with no
   transport is a **finding**, not an omission, and it belongs in `PLAN.md`
   before a line of code.

⚠ **One command's coverage is not the radio's settings.** The TM-D710 (#113)
shipped a 35-field settings schema built on its `MU` command, every field
measured on the radio and correct — and **no APRS at all**, on a radio whose
headline feature is APRS. `MU` carries menus 000-5xx; the APRS and TNC settings
are the 600-series and there is no `MU` parameter for one of them. The radio's
own `aprs_capable` flag was set to `true` in the same session. Nobody counted
the menus, so nobody noticed the settings stopped at 500.

The earlier note that "`MU` is not exhaustive — menus 504, 505 and 506 have no
parameter" was already in `FINDINGS.md`. It was read as a three-menu gap instead
of the question it actually was: *what else is missing, and how would we know?*

**Ask that question out loud in `PLAN.md`, with a number.** "The manual lists N
menus in G groups; this transport reaches M of them; the other N-M are <where>."

Then classify, because it decides how much of this process applies:

- **Clone of a family already supported** — AT-D868UV/D578 against the D890UV,
  ID-51/ID-5100 against the ID-52, FTM-300/400 against the FT5D. Container and
  protocol may carry over wholesale; addresses, enums and band limits must still
  be re-measured on the new model. Steps 3 and 5 shrink, steps 2 and 7 do not.
- **New card/file radio** — programmed by patching a file the radio itself wrote
  and will read back (FT5D, ID-52, TH-D75).
- **New cable radio** — either a whole-image clone (UV-5R, TD-H3) or
  record-by-record programming (AnyTone).

**Gate:** a `PLAN.md` in `scratchpad/<driver_key>/` naming the sources found,
the programming medium, the family, and what the user owns — **plus the menu
census above: how many menus the radio has, how many the chosen transport
reaches, and where the rest live.** Nothing is written before this exists; it is
also the thing that makes a resumed session cheap.

⚠ If the census cannot be completed because a group's transport is unknown, that
is the finding to report, not a detail to settle later. A radio shipped with a
whole feature's settings missing looks finished from the inside.

## 2. Anchor on a file the radio wrote

Before a line of encoder. The point is to have the radio's own output to measure
against, so that a published map can be *checked* rather than trusted.

Capture ritual (`templates/CAPTURE-CHECKLIST.md` is what the user follows):

1. A save straight off the radio, untouched.
2. A second save with **exactly one thing changed** in between.
3. A third save with **nothing** changed — this is the volatile-noise floor, and
   without it every later diff includes churn nobody can account for.

Then decode the published offsets against the real file and have the user
confirm a handful of values on the radio's own screen. Record the container
facts: exact path and extension on the card, file length, header length, and
whether anything looks like a checksum.

**Gate:** a `FINDINGS.md` showing the published map actually fits *this* radio.
If it does not, everything downstream changes, and finding that out here costs
an afternoon instead of a hardware session.

## 3. Container and memory encoder — pure Rust, no hardware

In `src-tauri/src/radios/<driver_key>/`:

- A container module that parses and re-emits the file, preserving the header
  byte-for-byte and handing out the body for patching. Guard the model and
  layout revision so a file from a different radio is refused with an error
  saying which.
- A memory module that encodes channels at the mapped offsets, and maps the
  app's channel lists onto whatever the radio has — banks, groups, zones. Check
  how many the radio allows per memory: most store exactly one, so a channel in
  two lists lands in the first.
- Never write to the user's own save. Write a new timestamped file beside it.
  A card file is a complete picture of their radio and these drivers keep no
  `.orig` backup.

**Gate:** re-encode the radio's own memories out of its own save and get
**byte-identical** output. This is the single most valuable test in the whole
process — it has caught real encoding bugs on every radio it was run against.
Add an encode→decode round-trip over the field matrix alongside it.

## 4. Seed the model and wire the path

Per `CONTRIBUTING.md`'s checklist. The parts that carry radio knowledge rather
than boilerplate:

- `tx_bands` from the manufacturer's spec; **`rx_bands` must model the
  receiver's real gaps**, not a single optimistic span. An out-of-coverage
  frequency does not error — it becomes a silently empty memory slot.
- Channel capacity, name length, and which of banks / zones / scan lists the
  radio genuinely has. A radio with program-scan limit pairs does not have
  named scan lists.
- The settings schema starts **empty on purpose** and is filled by step 6. A
  model does not ship without one, but seeding guesses is worse than seeding
  nothing.

**Gate:** run the app's own export pipeline against the dev database — not just
unit tests — and count what came out against what went in. An `#[ignore]`d,
env-gated harness in the driver folder is the established way to do this.

## 5. Measure the settings

The expensive half, and the one with the most ways to be quietly wrong.

### Instrument ladder — cheapest first

1. **Published research.** CHIRP sometimes has settings support even for a radio
   we only used it for channels; RE repos often carry menu tables.
2. **The manual.** Menu order, option lists, defaults. Do this *before* planning
   any measurement pass, so the passes are planned rather than exploratory.
3. **Whichever programmer is on hand** — RT Systems if the user owns it for this
   radio, otherwise the OEM CPS (CS-52, MCP-D75, ADMS, the AnyTone CPS, and so
   on). **No preference between them as instruments.** Ask; use what exists.
4. **The radio's front panel.** Slow for finding addresses, but it is the
   authority for values and it is always available.

### Rules that hold regardless of which tool it is

- **Validate the instrument before trusting it.** Diff a radio-authored save
  against the tool's re-save of the same data, and understand its normal churn
  before attributing a single byte to a setting.
- **Diff tool-vs-tool, never tool-vs-radio.** Cross-author diffs are swamped by
  the tool's own normalisation habits (blank fields written as `FF` where the
  radio writes spaces, revision stamps, working-copy churn).
- **Addresses from the tool, values confirmed on the radio.**
- **Establish which image the tool writes.** It is not always the one being
  patched: one programmer wrote the cable clone image while the app patches the
  microSD config file. The two overlapped, but that had to be *proven*.
- **Pin every setting with a unique `old -> new` value pair.** Two controls that
  both go `00 -> 01` in the same pass cannot be told apart, and attributing them
  by position has produced exactly-swapped fields.
- **The shipped app never depends on the CPS.** It is a measuring instrument
  during development and nothing more. No user is asked to own one.
- **Automation:** the MFC bridge in `scratchpad/*/rtsbridge.py` + `worker.ps1`
  drives Windows MFC controls generically and carries over to most CPS. A Qt or
  .NET programmer needs a different method, or manual passes with the user at
  the keyboard — resolve that in step 1, not here.
- **A programmer restarted mid-campaign reopens at defaults.** Reload the last
  capture and prove it with a pass showing zero bytes differ, or every
  attribution after that point is garbage.

**Gate:** a `MEASURED.md` where every row is graded — measured / inferred /
unverified-order — and rows the tool physically could not exercise are flagged
rather than smoothed over. `templates/MEASURED.md` has the shape.

## 6. Generate the table and schema; wire both ends

One measurement sheet → **both** the Rust field table and the profile form's
JSON schema, from one parse, by a script in `scratchpad/<key>/`. A form field
with no table entry silently does nothing; a table entry with no form field is
a setting nobody can reach. Assert the pair still agrees in a test.

Anything the generator cannot classify is reported and **not emitted**. Guessing
an encoding is the one failure mode that writes a wrong value to a real radio.

Then wire *both* ends, and check each off explicitly:

- [ ] the card-read (or cable-read) settings command, registered
- [ ] its `api.ts` binding
- [ ] the format's entry in `CARD_SETTINGS_READERS`
  (`src/components/profiles/ProfileEditor.tsx`) for a card radio
- [ ] **`apply_settings` called by the export path**
- [ ] the table↔schema agreement test

- [ ] **the coverage check against step 1's menu census** — the schema's field
      count and groups reconciled against the menus the radio actually has, with
      every absence named

**Gate:** a test proving an export carries memories **and** settings together,
and a **stated count**: N of the radio's M menus are exposed, and the M-N are
listed with a reason. "35 fields" is not a result; "35 of the 42 this transport
reaches, and the transport reaches 42 of the radio's ~90" is.

⚠ A cheap mechanical version of that reconciliation: the seed row already
asserts what the radio can do. A model with `aprs_capable: true` and no APRS
field in its settings schema is a contradiction the test suite can catch on its
own, and the TM-D710 shipped exactly that pairing for a whole session.

⚠ The fourth box is the one that nearly shipped broken. The read path worked and
the form filled correctly, so nothing looked wrong — the values simply never
reached the radio. A dead-code warning is what exposed it. Treat an "unused"
warning on an encoder as a bug report.

## 7. The hardware ladder

In order, stopping at the first failure. Each step asserts its property in the
file *before* the user is asked to load it, so a failure is the radio's answer
rather than a bad file. An `#[ignore]`d throwaway harness in the driver folder
is the established shape.

1. **Identity write** — write back a byte-identical copy of a radio save under a
   new name and load it. Proves the container with nothing at risk.
2. **One-byte write** — change a single memory name. **This is the checksum
   test**, and step 1 is not: an identical copy carries any internal digest
   along unchanged. If the radio rejects this and accepted step 1, there is a
   checksum to find.
3. **Full codeplug** — memories, groups, group names, per-group numbering.
   Check the memory list **in every group**, not just the one the radio powers
   up on.
4. **Band probe** — a channel at each edge of the claimed coverage, and one in
   any band the radio only receives. Count what actually landed against what
   the app reported.
5. **Settings spot check** — two visually obvious values (backlight, beep) read
   back off the radio's own screens. This is also where anything the programmer
   could not exercise in step 5 gets settled.

**Gate:** all five, on the real radio, reported honestly — including which steps
were skipped and why.

## 8. Land it

Verify in dev, commit, push. No PR — releases batch the accumulated branches.
Update `PROJECT_STATE.md` with where things stand and what is still open, and
record anything that would otherwise have to be re-taught next session.

---

## Worked examples

The two most recent builds, both complete enough to read as references:

- `scratchpad/thd75/PLAN.md` — the full arc, written before the work
- `scratchpad/thd75/rts/MEASURED.md` — sheet format, confidence grading, and the
  restart ritual for a measurement campaign spanning sessions
- `scratchpad/id52/CAPTURE-CHECKLIST.md` — the capture ritual as the user runs it
- `scratchpad/id52/RTS-SETTINGS-PLAN.md` — validating an instrument before
  trusting it, with the numbers

⚠ `scratchpad/` is gitignored. These exist only on the machine that made them;
if the folder is empty, the process above still stands on its own.

## Traps, each of which has already cost time

- ★ **A source's coverage is not the radio's.** Every field measured off one
  command can be right and the set still be badly incomplete — the TM-D710
  shipped a correct 35-field settings schema with no APRS on an APRS radio,
  because `MU` stops at menu 500 and nobody counted the menus. Enumerate what
  the radio HAS first, then hold every source against it.
- ★ **A noted gap is a question, not a footnote.** "`MU` is not exhaustive —
  three menus have no parameter" sat in the findings for two sessions. It was
  the same fact as "an entire feature is unreachable", written small.
- A working **read** path hides a dead **write** path. Verify the write.
- A printed option list is **display** order, not the stored index. One radio
  prints High/Medium/Low and stores Low as 0.
- Byte layout **does not** always follow menu order. It does for some
  manufacturers and not for others; measure, do not assume.
- An out-of-coverage frequency becomes a **silently empty** memory while the app
  reports success. Three repeaters were lost this way with a clean report.
- **Two samples agreeing is not a verified rule.** A 32-bit checksum read as
  16-bit fit both available samples, and cost four failed writes and a factory
  reset.
- **Third-party and OEM software both have defects.** One programmer could not
  write two end-of-list options at all, collapsing both to zero. Values seen
  only through a tool are unconfirmed until the radio shows them.
- **Never infer an address by analogy** with another radio, however similar.
  Prove it against an archived dump before writing it.
