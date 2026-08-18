# <Model> <container/protocol> — findings

Copy to `scratchpad/<driver_key>/FINDINGS.md`. This is the evidence log: what is
known, **how** it came to be known, and what is still assumed. Append; do not
rewrite history. When something turns out to be wrong, add a retraction section
rather than editing the original — a broken assumption is itself a finding, and
the next radio benefits from seeing how it broke.

Mark each section with its grade:

- **MEASURED** — changed on the radio (or in a validated tool) and observed
- **DECODED** — read out of a real file and confirmed against a known value
- **INFERRED** — consistent with the data but never moved
- **ASSUMED** — taken from a document, another model, or another radio's driver

---

## 1. Where the files live

Exact path on the card, extension, naming convention, file length. How the user
gets them off the radio.

## 2. The container

Header length and what is in it. Does it name the *writer* rather than the
radio? Body length. Anything that looks like a length, revision or digest.

## 3. Does the published map fit this radio?

The heart of step 2. For each region the published source claims, decode it out
of the real capture and show a value that proves it:

| Region | Published offset | What decoded | Grade |
|---|---|---|---|
| flags | | | |
| memories | | | |
| names | | | |
| groups | | | |

If something does not fit, say so here loudly — the plan changes.

## 4. The noise floor

Bytes that differ between two saves with nothing changed. Record the count and
the regions. A floor of zero is a gift; a floor of hundreds shapes every later
diff.

## 5. Groups / banks / zones

How membership is stored, and how many a memory can belong to. Where the group
names live. Whether a "current group" pointer exists — if it does, note it,
because leaving the radio pointed at an emptied group looks exactly like a
failed write.

## 6. Checksum

What is believed, and on what evidence. ⚠ An inherited "no checksum apparent"
from another model is **ASSUMED**, not measured. It is settled by the one-byte
write in the hardware ladder, not by the identity write — an identical copy
carries any digest along unchanged.

## 7. Ground truth — the encoder reproduces the radio's own memories

The byte-identical re-encode. Record how many memories, how many bytes compared,
and what differed (ideally nothing). Any deliberate exception belongs here with
its reason.

## 8. Still needed from the radio

The running list of questions only hardware can answer. Batch them — each entry
saves the user a trip.

- [ ] …

## 9. Hardware verification log

One subsection per ladder step, with the file name used, what was asserted
before it was handed over, and what the radio did. Record failures in full;
they are more useful than the passes.

### Step 1 — identity write

### Step 2 — one-byte write

### Step 3 — full codeplug

### Step 4 — band probe

### Step 5 — settings spot check

## 10. Retractions

Anything believed and later disproved, with what proved it. Keep these — every
radio so far has had at least one, and they are the most transferable part of
this document.

## Sources
