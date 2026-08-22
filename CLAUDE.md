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
- **CI is free and unmetered.** The repo went public on 2026-08-22, so standard
  runners cost nothing. Don't shape work around saving Actions minutes; the old
  rules here did, and they cost verification instead. CI now runs on pull
  requests, on pushes to `main`, and across all three OSes including macOS.
- **Open a PR for finished work.** One branch, one PR — CI checks it on all
  three OSes before it lands, which is the point: a branch that has never been
  verified anywhere but the author's Mac should not reach `main`. This reverses
  the old rule, which existed only because a PR cost metered minutes.
- **`main` is still verified on its own.** CI runs on push to `main` as well, so
  a merge of two green branches gets checked as the combination — this project
  has shipped bugs that existed nowhere else. Landing by local merge is still
  possible and still safe for something trivial, but the PR is the default.
- **The pre-push hook is still worth having.** `npm run ci` gives you the answer
  in 40 seconds instead of waiting on runners, but it is no longer the *only*
  macOS check, so a local toolchain that has drifted from CI's `stable` is no
  longer silently authoritative. `rustup update stable` when they disagree.
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
