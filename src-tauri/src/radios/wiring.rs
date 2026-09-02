//! Cross-cutting wiring checks for radio drivers (test-only).
//!
//! Adding a radio means touching a handful of places that the compiler cannot
//! connect for you: an encoder has to be *called* by the export, and a card
//! radio's format string has to appear in two TypeScript maps the Rust side
//! never sees. Both gaps are silent at runtime — the app builds, the UI draws,
//! and the failure only shows up on somebody's radio.
//!
//! These tests are deliberately crude: they read the source as text rather than
//! proving behaviour. That is the point. A behavioural test needs a fixture per
//! radio and so only ever covers the radios someone remembered to write one
//! for, where a source-level check covers the *next* radio automatically. Real
//! byte-level proof still belongs in each driver's own tests.
//!
//! See the `new-radio` skill for the process these guard.

use std::path::{Path, PathBuf};

/// `src-tauri/`, from which both the Rust and the frontend trees are reachable.
fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()))
}

/// Everything before the file's test module.
///
/// A call to an encoder from inside `mod tests` proves only that the encoder
/// was unit-tested, which is exactly the state the ID-52 and TH-D75 were both
/// in while their settings never reached the radio. Only production call sites
/// count.
fn production_half(src: &str) -> &str {
    match src.find("\n#[cfg(test)]") {
        Some(at) => &src[..at],
        None => src,
    }
}

/// Every `radios/<key>/` folder that holds a driver.
/// Every radio the app can actually program must be listed in the README's
/// "Supported radios" table.
///
/// ⚠ This exists because it was missed twice. The TH-D72 shipped in v26.8.29 and
/// never reached the table, and the BT-9000 was still sitting under "Planned"
/// under a manufacturer name that is not even its own on the day it shipped. A
/// radio someone owns and cannot tell is supported may as well not be.
///
/// Keyed on `display_name`, which is what the table prints and what the app
/// shows the operator, so the two cannot drift into describing it differently.
#[test]
fn every_seeded_radio_appears_in_the_readme() {
    let readme = std::fs::read_to_string(manifest_dir().join("../README.md"))
        .expect("README.md is readable from the crate");
    let supported = readme
        .split("## Supported radios")
        .nth(1)
        .expect("README has a `## Supported radios` heading")
        .split("\n## ")
        .next()
        .expect("that section ends at the next heading");

    let mut checked = 0;
    for d in crate::radios::registry::all_drivers() {
        let name = d.display_name();
        checked += 1;
        assert!(
            supported.contains(name),
            "{name} is a working driver but is not in the README's Supported radios \
             table. Someone who owns one cannot tell that it works."
        );
        // Bounded to the Planned TABLE, not to everything after the heading:
        // the credits and feature sections name shipped radios too, and an
        // unbounded match reported the AT-D890UV as still planned.
        let planned = readme
            .split("**Planned**")
            .nth(1)
            .and_then(|p| p.split("\n\n").find(|b| b.trim_start().starts_with('|')))
            .unwrap_or("");
        assert!(
            !planned.contains(name),
            "{name} ships AND is still listed under Planned"
        );
    }
    assert!(checked > 0, "no drivers found — this test would pass vacuously");
}

fn driver_dirs() -> Vec<PathBuf> {
    let radios = manifest_dir().join("src/radios");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&radios)
        .expect("radios/ is readable")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    assert!(!dirs.is_empty(), "found no driver folders under {}", radios.display());
    dirs
}

/// An encoder that writes settings into a radio image must be called by
/// something other than its own tests.
///
/// This is the failure that nearly shipped twice. `apply_settings` was written,
/// unit-tested and correct, the profile form filled in from the matching
/// decoder — and the export never called it, so every value the operator set
/// stayed in the database and none of it reached the radio. Nothing looked
/// broken from the outside, because the *read* path worked perfectly. What
/// eventually exposed it was a dead-code warning.
///
/// So: if a driver defines `apply_settings`, some other file in that driver's
/// folder has to call it outside a test module.
#[test]
fn every_settings_encoder_is_called_by_the_code_that_writes_the_radio() {
    let mut checked = 0;
    for dir in driver_dirs() {
        let settings = dir.join("settings.rs");
        if !settings.exists() {
            continue;
        }
        if !production_half(&read(&settings)).contains("fn apply_settings") {
            // Radios whose settings ride out over a cable rather than in an
            // exported file have no such encoder, and that is fine.
            continue;
        }
        checked += 1;

        let key = dir.file_name().unwrap().to_string_lossy().into_owned();
        let called_by: Vec<String> = std::fs::read_dir(&dir)
            .expect("driver folder is readable")
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|e| e == "rs")
                    && p.file_name().is_some_and(|n| n != "settings.rs")
                    && production_half(&read(p)).contains("apply_settings")
            })
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        assert!(
            !called_by.is_empty(),
            "radios/{key}/settings.rs defines apply_settings, but nothing outside it calls it. \
             The decoder and the profile form will still work, so this does not look broken — \
             the settings simply never reach the radio. Wire it into the export path."
        );
    }
    assert!(
        checked > 0,
        "no driver defines apply_settings any more — if that is deliberate, delete this test \
         rather than leaving it passing vacuously"
    );
}

/// Card formats offered by `find_memory_cards`, read out of its match arms.
fn card_formats() -> Vec<String> {
    let src = read(&manifest_dir().join("src/commands/program.rs"));
    let from = src
        .find("pub async fn find_memory_cards")
        .expect("find_memory_cards still exists in commands/program.rs");
    let body = &src[from..];
    let to = body
        .find("_ => Vec::new()")
        .expect("find_memory_cards still ends its match with a fallback arm");

    let formats: Vec<String> = body[..to]
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix('"')?;
            let (format, after) = rest.split_once('"')?;
            after.trim_start().starts_with("=>").then(|| format.to_string())
        })
        .collect();
    assert!(
        !formats.is_empty(),
        "parsed no formats out of find_memory_cards — the match was probably rewritten, so fix \
         this parser rather than deleting the test it feeds"
    );
    formats
}

/// A radio programmed from a memory card needs two entries in the frontend that
/// nothing on the Rust side can check: the format string is the key of both.
///
/// - `MEDIA_WRITES` supplies the front-panel steps the operator follows after
///   the file is written. Without it the card action offers no instructions.
/// - `CARD_SETTINGS_READERS` is what puts "Download from radio" in the profile
///   editor. Without it a radio that has a settings decoder has no way to reach
///   it, and the form comes up blank with no explanation.
///
/// There is no test runner on the frontend (no `test` script in `package.json`),
/// so this is checked from here, by reading the TypeScript as text.
#[test]
fn every_card_format_is_wired_into_the_frontend() {
    let root = manifest_dir().join("..");
    let media = read(&root.join("src/lib/radioProgramming.ts"));
    let editor = read(&root.join("src/components/profiles/ProfileEditor.tsx"));

    for format in card_formats() {
        assert!(
            media.contains(&format),
            "{format} offers a memory card in find_memory_cards but has no MEDIA_WRITES entry in \
             src/lib/radioProgramming.ts, so the operator is told nothing about what to do on the \
             radio once the file is written."
        );

        // Only a driver that can actually decode settings needs the reader; a
        // card radio whose settings are still unmeasured legitimately has none.
        let Some(key) = super::registry::all_drivers()
            .iter()
            .find(|d| {
                d.as_codeplug_exporter()
                    .is_some_and(|e| e.export_format() == format)
            })
            .map(|d| d.key())
        else {
            continue;
        };
        let settings = manifest_dir().join("src/radios").join(key).join("settings.rs");
        if !settings.exists() || !production_half(&read(&settings)).contains("fn decode_settings") {
            continue;
        }

        assert!(
            editor.contains(&format),
            "radios/{key} decodes settings from its card file, but {format} is missing from \
             CARD_SETTINGS_READERS in src/components/profiles/ProfileEditor.tsx — so the profile \
             editor offers no way to load them."
        );
    }
}

/// The profile form must decide "is this a card radio?" from how the radio is
/// PROGRAMMED, never from whether a settings reader happens to be wired yet.
///
/// That flag is what passes `defaults=false` into `seedValues`, which is the
/// single thing standing between a card radio and a form full of this app's
/// guesses — and the guess gets patched into the file the radio itself wrote.
///
/// The test above deliberately `continue`s past a driver with no
/// `decode_settings`, because a card radio whose settings are unmeasured
/// legitimately has no reader. That is exactly the state this one covers: it is
/// the normal intermediate state of every new-radio branch, and while
/// `isCardRadio` was keyed on `CARD_SETTINGS_READERS` it was also the state that
/// seeded ~300 defaults. Keying on the media-write map instead makes the flag
/// true from the moment the radio is programmed from a card, reader or not.
///
/// Read as text for the same reason as its neighbour — there is no test runner
/// on the frontend. (#90)
#[test]
fn the_profile_form_calls_a_card_radio_by_how_it_is_programmed() {
    let root = manifest_dir().join("..");
    let editor = read(&root.join("src/components/profiles/ProfileEditor.tsx"));

    let decl = editor
        .lines()
        .find(|l| l.trim_start().starts_with("const isCardRadio"))
        .unwrap_or_else(|| {
            panic!(
                "no `const isCardRadio` in ProfileEditor.tsx. It is what passes defaults=false \
                 into seedValues — if it was renamed, repoint this guard rather than deleting it."
            )
        });
    assert!(
        decl.contains("mediaWriteForFormat"),
        "isCardRadio is derived from `{}`. It must come from mediaWriteForFormat (how the radio \
         is programmed), not from CARD_SETTINGS_READERS (whether a decoder is wired yet) — \
         otherwise a card radio with a settings schema and no decoder seeds every field with a \
         schema default and Save patches this app's guesses into the operator's own file.",
        decl.trim()
    );
    assert!(
        !decl.contains("CARD_SETTINGS_READERS") && !decl.contains("cardReader"),
        "isCardRadio is derived from the settings-reader map: `{}`",
        decl.trim()
    );

    // And the flag has to still be the thing that suppresses the defaults.
    assert!(
        editor.contains("!isCardRadio"),
        "ProfileEditor.tsx no longer passes !isCardRadio as seedValues' `defaults` argument — the \
         flag this test guards is not reaching the seeding decision."
    );
}

/// Every Tauri command that takes a serial `port` must claim it (#67).
///
/// The claim is one line and easy to leave out of a new command, and leaving it
/// out is invisible: the command works perfectly on its own and only misbehaves
/// when a second operation overlaps it — exactly the case nobody tests by hand.
/// So the rule is checked mechanically instead.
///
/// The invariant is deliberately "takes a port", not "does radio I/O", because
/// the second is not decidable by reading text. A command that genuinely takes
/// a port name without talking to it can opt out with the marker named in the
/// failure message, which makes the exception a thing someone WROTE rather than
/// a thing they forgot.
#[test]
fn every_command_taking_a_port_claims_it() {
    const OPT_OUT: &str = "no-port-lock:";
    let dir = manifest_dir().join("src/commands");
    let mut checked = 0;

    for entry in std::fs::read_dir(&dir).expect("commands dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let src = read(&path);
        let file = path.file_name().unwrap().to_string_lossy().to_string();

        // Split on the attribute so each chunk is exactly one command body.
        for chunk in src.split("#[tauri::command]").skip(1) {
            let Some(sig_end) = chunk.find(')') else { continue };
            let sig = &chunk[..sig_end];
            let Some(name) = sig
                .split("fn ")
                .nth(1)
                .and_then(|r| r.split('(').next())
                .map(str::trim)
            else {
                continue;
            };
            if !sig.contains("port: String") {
                continue;
            }
            if chunk.contains(OPT_OUT) {
                continue;
            }
            assert!(
                chunk.contains("port_lock::claim"),
                "{file}: `{name}` takes a serial port but never calls \
                 radios::port_lock::claim, so a second radio operation can start on that \
                 port while it runs. Claim it as the first line inside the spawn_blocking \
                 closure, so the guard lives for the whole session. If this command really \
                 does not talk to the radio, say so with a `// {OPT_OUT} <why>` comment."
            );
            checked += 1;
        }
    }

    assert!(
        checked >= 10,
        "only {checked} port-taking commands found — the parser has probably stopped \
         matching, so fix it rather than deleting the test it feeds"
    );
}

/// Split a source file's production half into one chunk per top-level `fn`,
/// each chunk running from its signature to the next one.
fn top_level_fns(src: &str) -> Vec<(String, String)> {
    let src = production_half(src);
    let mut starts: Vec<usize> = Vec::new();
    for (nl, _) in src.match_indices('\n') {
        let rest = &src[nl + 1..];
        if ["fn ", "pub fn ", "async fn ", "pub async fn "]
            .iter()
            .any(|kw| rest.starts_with(kw))
        {
            starts.push(nl + 1);
        }
    }
    let mut out = Vec::new();
    for (n, &from) in starts.iter().enumerate() {
        let to = starts.get(n + 1).copied().unwrap_or(src.len());
        let body = &src[from..to];
        let name = body
            .split("fn ")
            .nth(1)
            .and_then(|s| s.split(['(', '<']).next())
            .unwrap_or("?")
            .to_string();
        out.push((name, body.to_string()));
    }
    out
}

/// Anything that hands a profile's settings to a driver checks their range
/// first.
///
/// The schema's `min`/`max` were decoration for the whole life of the project:
/// the form rendered them as HTML attributes, which flag an out-of-range value
/// without preventing one, and each driver's encoder cast whatever arrived
/// straight down to a byte — a UV-5R backlight timeout of 300 reached the radio
/// as byte 44 (#87). The check is one call, `settings_bounds::check_settings_bounds`,
/// and it has to run in front of *every* path, not the two someone remembered.
///
/// Crude on purpose, like the rest of this file: it reads the source for the
/// three shapes that carry settings to a radio — an `ImageProgramRequest`, an
/// `ExportRequest`, and a direct `write_settings` — and requires the same
/// function to mention the check. A fourth path added later is caught the day
/// it is written rather than the day it programs somebody's radio.
#[test]
fn every_path_that_sends_settings_to_a_radio_checks_their_range_first() {
    const CARRIES_SETTINGS: [&str; 3] = ["ImageProgramRequest {", "ExportRequest {", ".write_settings("];
    let mut checked = 0;
    for file in ["src/commands/program.rs", "src/commands/export.rs"] {
        let path = manifest_dir().join(file);
        for (name, body) in top_level_fns(&read(&path)) {
            let Some(marker) = CARRIES_SETTINGS.iter().find(|m| body.contains(**m)) else {
                continue;
            };
            checked += 1;
            assert!(
                body.contains("strip_out_of_range"),
                "{file}: {name} carries settings to a radio (`{marker}`) but never calls \
                 strip_out_of_range. \
                 Every value it carries is about to be cast down to a byte by a driver's \
                 encoder, so an out-of-range one lands on the radio as a different setting \
                 and nothing reports it."
            );
        }
    }
    assert_eq!(
        checked, 3,
        "expected the three settings-carrying paths (settings write, codeplug program, card \
         export) — found {checked}. If a path moved, point this test at it rather than \
         leaving it passing vacuously"
    );
}
