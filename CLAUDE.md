# CodePlug Magic — working notes for Claude

A Tauri v2 desktop app (React + TypeScript frontend, Rust backend, SQLite via
`sqlx`) that builds ham-radio codeplugs and programs radios over USB, over a
serial cable, or through the microSD card the radio itself writes.

`CONTRIBUTING.md` is the contributor-facing guide — tech stack, setup, project
layout, and the wiring checklist for a new driver. It is accurate; read it
rather than re-deriving any of it.

## Adding or extending a radio

**Load the `new-radio` skill before starting.** It carries the eight-step
process the last three radios were built with, the gate that has to be true
before each step, and the traps that have already cost real hardware time —
including a failure that reached a physical radio and one that caused a factory
reset. Reading it first is cheaper than rediscovering any of them.

That applies to a new-radio issue (they carry the `new-radio` label), and to any
substantial work under `src-tauri/src/radios/`.

## Standing workflow

- **Verify in dev, then commit, then push.** `npm run tauri:dev` runs against a
  separate `.dev` app identifier, so dev never shares the production database.
- **Don't open a PR to land finished work.** Push the branch and leave it.
  PRs exist to batch accumulated branches when a release is cut — the free
  Actions allowance is metered, CI runs on pull requests only, and there is no
  hosted macOS leg, so the pre-push hook (`npm run ci`) is the macOS check.
  "Done" means pushed, not merged.
- **Never run `cargo fmt`.** It reformats 44 files and nothing in CI or the hook
  checks formatting. Hand-format new code to match its surroundings.
- **Migrations are immutable once applied.** Editing one panics the app at
  startup with a checksum mismatch. Add the next-numbered file instead.
- **`scratchpad/` is gitignored** — reverse-engineering notes, radio captures
  and measurement sheets live there and exist only on the machine that made
  them. Anything that must survive belongs in committed source or in a doc.

## Hardware claims

Nothing about a radio is verified until the radio says so. A protocol note
inherited from another model, a value read back from the tool that wrote it, and
two samples that happen to agree are all unverified — each of those three has
been wrong here. Say plainly which parts of a change were confirmed on hardware
and which were not.
