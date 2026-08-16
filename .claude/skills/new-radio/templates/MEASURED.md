# <Model> settings — measured addresses

Copy to `scratchpad/<driver_key>/MEASURED.md`. This is the sheet the generator
parses, so its tables are source code as much as notes. Keep the column shape
consistent — a ragged row is reported and **not emitted**, which is the intended
behaviour.

## ★ RESUME HERE

State, in one paragraph: which tabs/sections are complete, how many controls
remain, and the last good capture file name. A session that picks this up cold
should not have to re-derive any of it.

**Instrument:** <RT Systems / OEM CPS / front panel> — and whether it has been
validated (see below).

### Restart ritual

1. **User:** <what they start — VM, programmer, terminal>
2. **Claude:** <what to start, in order, and how to check it is listening>
3. **Reload the last capture before anything else**, then prove the baseline
   with a save-only pass showing **zero bytes differ**.

⚠ A programmer restarted mid-campaign reopens its document at **defaults**, not
at the state that produced the last capture. Diffing against a stale baseline
attributes garbage to every byte. The zero-diff pass is what makes the baseline
trustworthy, and it is not optional.

### Things that must NOT become plain schema fields

Anything the instrument could not exercise, or that is not stored at all. Each
needs the radio to settle it, or needs leaving out.

| item | address | why it is unsettled |
|---|---|---|
| | | tool cannot write this value |
| | | inferred from a default, never moved |
| | | not stored anywhere |

---

## Confidence grading

Every row carries one:

- **M** — measured: this control was moved and this byte changed
- **E** — enum measured: every option was walked and its stored value recorded
- **U** — unverified order: address and width are measured, but option labels
  came from the manual's **printed** order with nothing to check them against.
  ⚠ Printed order is *display* order and is not always the stored index.
- **I** — inferred: consistent with the current value, never moved

The generator emits **U** and **I** rows with a marker, never silently mixed in.

## Address tables

One section per tab or menu group. Body offsets (file offset minus the header)
so they match what the encoder uses — say which, once, here.

### <Tab name> — <COMPLETE / n of m>

| addr | control | encoding | grade |
|---|---|---|---|
| `0x0000` | | `Off=0 On=1` | M |
| `0x0000` | | `0..60` raw seconds | M |
| `0x0000` | | index, `A=0 B=1 …` | E |

### Encoding shapes seen on this radio

List them explicitly — the generator has to handle each, and a shape nobody
wrote down is a shape that gets guessed:

- whole-byte enum, index in the tool's order
- raw numeric (seconds, knots, character counts — no scaling)
- boolean 0/1
- **bitfields** — one byte holding several flags; list every such byte, because
  none of them has ever been predictable from the UI layout
- fixed-length ASCII — note the padding: NUL, space, and `0xFF` have all
  appeared, and getting it wrong corrupts every short value
- offsets and gapped enums — where a control writes into a shared enum with
  values it cannot itself express. ⚠ A round trip must **preserve** an
  unrecognised code, never normalise it

## Records that are not flat fields

Repeating structures — status texts, phrase lists, beacon objects. These need a
different shape than the flat field table and are a design decision, not
transcription. Note whether each holds per-row flags or a **selector index**;
mistaking one for the other is the usual trap.

## Instrument validation

The evidence that this tool can be trusted for this radio:

| comparison | bytes differing |
|---|---|
| radio-authored save vs tool re-save, whole file | |
| …restricted to the region being measured | |

Plus: which image the tool actually writes, and the proof that it overlaps the
one being patched. And at least one address confirmed independently against the
radio's own screen.
