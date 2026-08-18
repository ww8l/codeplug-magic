# <Model> capture checklist — issue #NN

Copy to `scratchpad/<driver_key>/CAPTURE-CHECKLIST.md`. This is the sheet the
**user** works from at the radio, so write it in their terms: menu paths as the
radio words them, file names spelled out, nothing assumed.

Everything the driver needs that cannot be read out of a manual. **Drop every
file into `scratchpad/<driver_key>/` under the exact names below** — that folder
is gitignored, so a personal radio's contents never reach the repo.

Getting files off the radio: <the shortcut, e.g. front-panel USB mass-storage
mode so the card never has to come out — spell out the menu path>. While in
there, note **what the mounted volume is called**, so the app can offer "that
card, there" instead of a file-hunt.

---

## Step 1 — capture the radio exactly as it is now

The baseline. Take it before touching anything.

<Menu path> → save →

    <key>_01_base.<ext>

If the radio also exports a **human-readable** form of the same data (a CSV of
memories, say), take that too, back to back with the binary and without touching
anything in between:

    <key>_01_base.csv

That pairing is worth a lot: the text says in plain language what the binary
says in hex, which is what lets records be *located* rather than guessed at.

## Step 2 — the noise floor

Save **again immediately**, changing nothing at all:

    <key>_02_noise.<ext>

Diffing 01 against 02 shows what the radio rewrites on every save regardless —
clocks, counters, working-copy scratch. Without this, every later diff includes
churn nobody can account for. It takes thirty seconds and it has mattered on
every radio so far.

## Step 3 — one change, one save

Change **exactly one** thing, something unambiguous and easy to describe — a
memory name, or one menu item whose current value is known. Write down what was
changed, from what, to what.

    <key>_03_<what-changed>.<ext>

    Changed: <menu item / memory N field>
    From:    <old value>
    To:      <new value>

One change per save is the whole method. Two controls moved in one save cannot
be told apart afterwards.

## Step 4 — a probe set (optional but cheap)

A handful of memories chosen to exercise encodings a normal codeplug does not:

- one in each band the radio covers, including any receive-only band
- one at each edge of coverage
- a repeater with an odd split and a tone
- one with the longest name the radio allows, and one with the shortest
- two in **different** banks/groups, to pin how membership is stored

Then save again as `<key>_04_probe.<ext>`.

---

## What to report back

- The exact folder path and file extension on the card
- File length in bytes
- Whether the radio accepted every step without complaint
- Anything the radio displayed that was surprising

## Still needed from the radio later

Keep this list current as questions come up — it is what the next hardware
session is for, and batching them saves the user a trip.

- [ ] …
