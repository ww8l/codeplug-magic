## What this changes

A short description of the change and why.

Closes #<!-- issue number, if applicable -->

## How it was tested

- [ ] `cargo test` passes
- [ ] `cargo clippy --all-targets` is clean
- [ ] `npm run build` (tsc + vite) is clean
- [ ] Ran the app (`npm run tauri dev`) and exercised the change

## Hardware verification (if this touches a radio read/write path)

- Radio model tested:
- What was verified: (e.g. read → program → byte-verify, settings round-trip)
- N/A — this change doesn't touch a radio path

## Checklist

- [ ] No existing migration file was edited (added a new numbered one if schema changed)
- [ ] Change is focused and matches surrounding code style
- [ ] Updated docs / README / CONTRIBUTING if behavior or setup changed
