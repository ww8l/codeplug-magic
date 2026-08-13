//! Icom ID-52 non-channel settings ("radio profile"): the field table plus
//! decode and apply against the `.icf` memory image.
//!
//! Like the FT5D and unlike every cable radio here, these settings never travel
//! over a wire. They are read out of an `.icf` the radio wrote to its own
//! microSD card and written back into it, so this module is pure byte
//! manipulation — see [`super::icf`] for the container and [`super::memory`]
//! for the same arrangement applied to channels.
//!
//! ## Where the offsets came from
//!
//! Nowhere published. CHIRP has no ID-52 settings map, Icom documents no file
//! format, and the one thing that *is* published — the Advanced Manual — gives
//! names, menu paths, defaults and option lists but no addresses. So every
//! offset in [`ID52_SETTINGS_FIELDS`] was **measured**, one save-and-diff cycle
//! at a time, by driving RT Systems' programmer inside a VM and diffing the
//! `.icf` it wrote against the previous one. Roughly 50 passes over eight
//! sessions; the working sheet is `scratchpad/id52/PASS3-SHEET.md` (gitignored —
//! it quotes dumps of a personal radio).
//!
//! The table is GENERATED from that sheet by `scratchpad/id52/gen_id52_table.py`,
//! which joins the measured addresses against the manual's option lists, so the
//! encoder and the profile form's schema cannot drift apart. Do not hand-edit
//! `id52_settings_table.rs`; regenerate it.
//!
//! ## What the measurement can and cannot vouch for
//!
//! **Addresses** are solid: each was produced by changing exactly one control
//! and observing exactly one byte move, and every prediction the layout made
//! about the *next* field's address has landed.
//!
//! **Values** are one step weaker, because the instrument is third-party
//! software rather than the radio. RT Systems has been caught four times
//! writing something other than what it displays — see [`self`]'s sibling notes
//! on `rx-callsign-display`, the two D-PRS `TimeStamp` fields and D-PRS speed.
//! Anything whose stored value has been confirmed only through RT Systems is
//! marked `RTS-ONLY` in the generator's source and carries a comment here. Those
//! need one capture taken off the radio itself before they can be trusted.
//!
//! ## Encodings peculiar to this radio
//!
//! - Multi-byte integers are **big-endian**, unlike the AnyTone's little-endian
//!   settings block.
//! - Text is **space-padded** (`0x20`), not NUL- or `0xFF`-padded. A blank
//!   43-byte comment is 43 spaces.
//! - One byte, `0x03CAA3`, packs unrelated flags as bits and must be
//!   read-modify-written; six of its eight bits are still unidentified, so
//!   assigning the whole byte would clear settings nobody has named yet.

// Nothing calls this module yet: the profile-editor path needs a migration that
// seeds the schema, the two capability flags, and a command to read and write a
// card file. Until that lands, `IcomId52` keeps advertising export-only — a
// capability claimed before it works becomes a button that fails on the radio.
// Remove this when the command layer arrives.
#![allow(dead_code)]

use serde_json::{Map, Value};

/// One decodable setting: where it lives in the decoded `.icf` image and how to
/// read it. Values are shaped like the profile form — booleans, numbers, select
/// labels or text.
pub(crate) struct SF {
    pub key: &'static str,
    /// Byte offset into the decoded image (`IcfFile::image`), absolute.
    pub byte: u32,
    pub kind: SK,
}

pub(crate) enum SK {
    /// Unsigned **big-endian** integer of `width` bytes.
    Uint { width: u8 },
    /// Signed **big-endian**, two's complement, of `width` bytes. Used by the
    /// UTC offset, which is minutes east of Greenwich and legitimately negative
    /// (`FD C6` = −570 = UTC−09:30).
    Int { width: u8 },
    /// Whole 1-byte flag. Checkbox polarity is uniform on this radio —
    /// off = 0, on = 1 — which held across every pass without one exception.
    Bool,
    /// Single bit at `shift` within a byte, read-modify-written so the bits
    /// sharing that byte survive. Only `0x03CAA3` needs this so far.
    BoolBit { shift: u8 },
    /// Enumerated whole-value mapped to a label.
    Enum {
        width: u8,
        labels: &'static [(u32, &'static str)],
    },
    /// Fixed-length ASCII, space-padded.
    Text { len: u32 },
}

include!("id52_settings_table.rs");

fn read_uint(image: &[u8], at: usize, width: u8) -> u32 {
    let mut v = 0u32;
    for i in 0..width as usize {
        v = (v << 8) | *image.get(at + i).unwrap_or(&0) as u32;
    }
    v
}

/// Sign-extend a big-endian two's-complement field of `width` bytes.
fn read_int(image: &[u8], at: usize, width: u8) -> i64 {
    let raw = read_uint(image, at, width) as u64;
    let bits = 8 * width as u32;
    let sign = 1u64 << (bits - 1);
    if raw & sign != 0 {
        (raw as i64) - (1i64 << bits)
    } else {
        raw as i64
    }
}

fn label_for(labels: &[(u32, &'static str)], raw: u32) -> Value {
    match labels.iter().find(|(v, _)| *v == raw) {
        Some((_, l)) => Value::String((*l).to_string()),
        // Pass an unrecognised value through as its number rather than
        // mislabelling it. This is not hypothetical: `GPS Select` turned out to
        // have a fourth slot the manual does not document, and it read as a
        // number here for several passes before it was named.
        None => Value::String(raw.to_string()),
    }
}

/// Read one field out of the image, shaped like the profile form.
fn decode_field(image: &[u8], f: &SF) -> Value {
    let at = f.byte as usize;
    match &f.kind {
        SK::Uint { width } => Value::from(read_uint(image, at, *width)),
        SK::Int { width } => Value::from(read_int(image, at, *width)),
        SK::Bool => Value::Bool(image[at] != 0),
        SK::BoolBit { shift } => Value::Bool((image[at] >> shift) & 1 == 1),
        SK::Enum { width, labels } => label_for(labels, read_uint(image, at, *width)),
        SK::Text { len } => {
            let end = (at + *len as usize).min(image.len());
            let raw = &image[at..end];
            // Space-padded, but tolerate a NUL terminator: the radio writes
            // spaces and RT Systems has been seen to write both.
            let cut = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
            Value::String(
                String::from_utf8_lossy(&raw[..cut])
                    .trim_end()
                    .to_string(),
            )
        }
    }
}

/// Decode every known setting out of a decoded `.icf` image, shaped like the
/// profile form. Unknown or out-of-range enum values come back as their raw
/// number rather than a wrong label.
pub(crate) fn decode_settings(image: &[u8]) -> Value {
    let mut out = Map::new();
    for f in ID52_SETTINGS_FIELDS {
        if field_end(f) > image.len() {
            continue;
        }
        out.insert(f.key.to_string(), decode_field(image, f));
    }
    Value::Object(out)
}

/// Last byte a field touches, exclusive — used to keep a short image from
/// panicking a decode.
fn field_end(f: &SF) -> usize {
    let at = f.byte as usize;
    at + match &f.kind {
        SK::Uint { width } | SK::Int { width } | SK::Enum { width, .. } => *width as usize,
        SK::Bool | SK::BoolBit { .. } => 1,
        SK::Text { len } => *len as usize,
    }
}

/// Write a profile's settings into a decoded `.icf` image, returning how many
/// fields were applied. Keys the table does not know, and values of the wrong
/// shape, are skipped rather than guessed at — a profile saved against an older
/// schema must not corrupt the image.
pub(crate) fn apply_settings(image: &mut [u8], settings: &Map<String, Value>) -> usize {
    let mut written = 0;
    for f in ID52_SETTINGS_FIELDS {
        let Some(v) = settings.get(f.key) else {
            continue;
        };
        if field_end(f) > image.len() {
            continue;
        }
        // A field whose stored value already matches is left ALONE, not
        // rewritten with the same value. Re-encoding is not byte-neutral —
        // this writer pads text with spaces, and a file that came from RT
        // Systems may be NUL-padded — so "unchanged" has to mean "no bytes
        // touched", or a save would rewrite the tail of every string field the
        // operator never opened. The FT5D learned this by blanking a callsign.
        if decode_field(image, f) == *v {
            written += 1;
            continue;
        }
        let at = f.byte as usize;
        let ok = match &f.kind {
            SK::Uint { width } => match v.as_u64() {
                Some(n) => {
                    write_uint(image, at, *width, n as u32);
                    true
                }
                None => false,
            },
            SK::Int { width } => match v.as_i64() {
                Some(n) => {
                    write_int(image, at, *width, n);
                    true
                }
                None => false,
            },
            SK::Bool => match v.as_bool() {
                Some(b) => {
                    image[at] = b as u8;
                    true
                }
                None => false,
            },
            SK::BoolBit { shift } => match v.as_bool() {
                Some(b) => {
                    let mask = 1u8 << shift;
                    image[at] = (image[at] & !mask) | if b { mask } else { 0 };
                    true
                }
                None => false,
            },
            SK::Enum { width, labels } => match raw_for(labels, v) {
                Some(raw) => {
                    write_uint(image, at, *width, raw);
                    true
                }
                None => false,
            },
            SK::Text { len } => match v.as_str() {
                Some(s) => {
                    write_text(image, at, *len as usize, s);
                    true
                }
                None => false,
            },
        };
        if ok {
            written += 1;
        }
    }
    written
}

/// The stored number behind a form value: a label from the option list, or the
/// passthrough decimal string [`decode_settings`] emits for unknown values.
fn raw_for(labels: &[(u32, &'static str)], v: &Value) -> Option<u32> {
    if let Some(s) = v.as_str() {
        if let Some((raw, _)) = labels.iter().find(|(_, l)| *l == s) {
            return Some(*raw);
        }
        return s.parse().ok();
    }
    v.as_u64().map(|n| n as u32)
}

fn write_uint(image: &mut [u8], at: usize, width: u8, v: u32) {
    for i in 0..width as usize {
        let shift = 8 * (width as usize - 1 - i);
        image[at + i] = ((v >> shift) & 0xFF) as u8;
    }
}

fn write_int(image: &mut [u8], at: usize, width: u8, v: i64) {
    let bits = 8 * width as u32;
    let wrapped = (v as u64) & (u64::MAX >> (64 - bits));
    write_uint(image, at, width, wrapped as u32);
}

/// ASCII, space-padded — what the radio itself writes. Text longer than the
/// field is truncated, matching what RT Systems' own save does rather than
/// refusing the value.
fn write_text(image: &mut [u8], at: usize, len: usize, s: &str) {
    for i in 0..len {
        let ch = s.as_bytes().get(i).copied().unwrap_or(b' ');
        // Non-ASCII would be written as some other byte entirely; a space is
        // the safe substitution, and the radio's own keyboard cannot produce
        // one anyway.
        image[at + i] = if ch.is_ascii_graphic() || ch == b' ' {
            ch
        } else {
            b' '
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key is unique and every address is inside the image. A duplicate
    /// key silently shadows a setting in the form; an address past the end
    /// would be dropped at decode and never noticed.
    #[test]
    fn table_is_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for f in ID52_SETTINGS_FIELDS {
            assert!(seen.insert(f.key), "duplicate settings key {}", f.key);
            assert!(
                field_end(f) <= super::super::ID52_IMAGE_LEN,
                "{} runs past the end of the image",
                f.key
            );
        }
    }

    /// Enum option lists must not repeat a label or a raw value: `raw_for` picks
    /// the first match, so a duplicate label would make one option unreachable
    /// and a duplicate raw would make the decode ambiguous.
    #[test]
    fn enum_labels_are_unambiguous() {
        for f in ID52_SETTINGS_FIELDS {
            if let SK::Enum { labels, .. } = &f.kind {
                let mut raws = std::collections::HashSet::new();
                let mut names = std::collections::HashSet::new();
                for (raw, label) in *labels {
                    assert!(raws.insert(raw), "{}: duplicate value {}", f.key, raw);
                    assert!(names.insert(label), "{}: duplicate label {}", f.key, label);
                }
            }
        }
    }

    fn blank_image() -> Vec<u8> {
        vec![0u8; super::super::ID52_IMAGE_LEN]
    }

    /// Big-endian, not little — the byte order that separates this radio from
    /// the AnyTone block. `03CAB3` is the UTC offset in minutes, and the value
    /// measured off the radio is `FD C6` = −570 = UTC−09:30.
    #[test]
    fn utc_offset_is_signed_big_endian_minutes() {
        let mut img = blank_image();
        img[0x03CAB3] = 0xFD;
        img[0x03CAB4] = 0xC6;
        let f = SF {
            key: "utc-offset",
            byte: 0x03CAB3,
            kind: SK::Int { width: 2 },
        };
        assert_eq!(decode_field(&img, &f), Value::from(-570));

        let mut back = blank_image();
        let mut m = Map::new();
        m.insert("utc-offset".into(), Value::from(-570));
        apply_one(&mut back, &f, &m);
        assert_eq!(&back[0x03CAB3..=0x03CAB4], &[0xFD, 0xC6]);
    }

    /// Text is space-padded, and a blank field is spaces rather than NULs.
    #[test]
    fn text_round_trips_space_padded() {
        let f = SF {
            key: "dprs-item-name",
            byte: 0x03CC1A,
            kind: SK::Text { len: 9 },
        };
        let mut img = blank_image();
        let mut m = Map::new();
        m.insert("dprs-item-name".into(), Value::from("ITEM1"));
        apply_one(&mut img, &f, &m);
        assert_eq!(&img[0x03CC1A..0x03CC23], b"ITEM1    ");
        assert_eq!(decode_field(&img, &f), Value::from("ITEM1"));
    }

    /// Overlong text is truncated at the field width, which is what the radio
    /// and RT Systems both do — a 50-character comment stored 43 bytes.
    #[test]
    fn text_is_truncated_at_the_field_width() {
        let f = SF {
            key: "c",
            byte: 0x1000,
            kind: SK::Text { len: 4 },
        };
        let mut img = blank_image();
        let mut m = Map::new();
        m.insert("c".into(), Value::from("ABCDEFGH"));
        apply_one(&mut img, &f, &m);
        assert_eq!(&img[0x1000..0x1004], b"ABCD");
    }

    /// The `03CAA3` flags byte must be read-modify-written. Six of its eight
    /// bits have never been identified, so assigning the byte would silently
    /// clear settings that have no name yet.
    #[test]
    fn bit_field_preserves_its_neighbours() {
        let f = SF {
            key: "att-band-a",
            byte: 0x03CAA3,
            kind: SK::BoolBit { shift: 7 },
        };
        let mut img = blank_image();
        img[0x03CAA3] = 0b0101_1010; // unrelated bits, including b6 = ATT (FM)
        let mut m = Map::new();
        m.insert("att-band-a".into(), Value::Bool(true));
        apply_one(&mut img, &f, &m);
        assert_eq!(img[0x03CAA3], 0b1101_1010, "neighbouring bits were clobbered");
    }

    /// A value that already matches must not rewrite its bytes. Re-encoding is
    /// not byte-neutral for text, so "no change" has to mean "nothing touched".
    #[test]
    fn unchanged_text_is_not_rewritten() {
        let f = SF {
            key: "c",
            byte: 0x1000,
            kind: SK::Text { len: 6 },
        };
        let mut img = blank_image();
        // NUL-padded, as a file from other software may well be.
        img[0x1000..0x1006].copy_from_slice(b"WW8L\0\0");
        let mut m = Map::new();
        m.insert("c".into(), Value::from("WW8L"));
        apply_one(&mut img, &f, &m);
        assert_eq!(
            &img[0x1000..0x1006],
            b"WW8L\0\0",
            "an unchanged field was re-padded"
        );
    }

    /// An unrecognised enum value decodes to its number and survives a
    /// round-trip, so an option this table has not named yet is preserved
    /// rather than reset to the first label.
    #[test]
    fn unknown_enum_value_round_trips_as_a_number() {
        let f = SF {
            key: "e",
            byte: 0x1000,
            kind: SK::Enum {
                width: 1,
                labels: &[(0, "Off"), (1, "On")],
            },
        };
        let mut img = blank_image();
        img[0x1000] = 7;
        assert_eq!(decode_field(&img, &f), Value::from("7"));
        let mut m = Map::new();
        m.insert("e".into(), Value::from("7"));
        apply_one(&mut img, &f, &m);
        assert_eq!(img[0x1000], 7);
    }

    /// Decode a real `.icf` and check it against the settings the radio's own
    /// screens were showing when it was saved.
    ///
    /// This is the test that actually validates the table. Everything else here
    /// checks the *engine*; only this checks that 156 measured addresses point
    /// at the settings they claim to. The expectations were read off the RT
    /// Systems screenshots taken beside each capture — `auto23-set.png` and the
    /// full-screen shots of the Common tab — not off the table being tested.
    ///
    /// The spread is deliberate: assertions are drawn from every address
    /// cluster (`03C9Bx`, `03C9Ex`, `03CAxx`, `03E93x`). A misaligned map
    /// displaces a whole run, so one probe per cluster catches what a dozen
    /// probes in one cluster would miss.
    ///
    /// `#[ignore]`d because it needs the captures under `scratchpad/`, which is
    /// gitignored and cannot exist in CI:
    ///
    ///     cargo test --lib icom_id52::settings -- --ignored --nocapture
    #[test]
    #[ignore = "needs a real .icf under scratchpad/id52/"]
    fn decodes_a_real_capture_to_the_values_the_radio_was_showing() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scratchpad/id52/auto23.icf"
        );
        let Ok(text) = std::fs::read_to_string(path) else {
            eprintln!("skipped: no capture at {path}");
            return;
        };
        let icf = super::super::icf::IcfFile::parse(&text).expect("a real ID-52 file parses");
        let got = decode_settings(icf.image());
        let got = got.as_object().expect("decode returns an object");

        // (key, expected) — every one read off a screenshot of that capture.
        let expect: &[(&str, Value)] = &[
            // Common tab, Set Mode box
            ("auto-power-off", Value::from("120")),
            ("time-out-timer", Value::from("30")),
            ("active-band", Value::from("Ham")),
            ("power-save", Value::from("Long")),
            ("key-lock", Value::from("All")),
            ("monitor", Value::from("Push")),
            ("ptt-lock", Value::Bool(false)),
            ("busy-channel-lockout", Value::Bool(true)),
            ("dial-speedup", Value::Bool(true)),
            ("mic-gain-internal", Value::from("1")),
            ("mic-gain-external", Value::from("2")),
            // Squelch: A was 9, B was Open — the "Level n = n+1" expansion.
            ("squelch-a", Value::from("9")),
            ("squelch-b", Value::from("Open")),
            // Display box
            ("busy-led", Value::Bool(false)),
            ("opening-message", Value::Bool(true)),
            ("voltage-indication", Value::Bool(false)),
            ("dim-screen", Value::Bool(true)),
            ("backlight", Value::from("Off")),
            ("brightness", Value::from("5")),
            ("scroll-speed", Value::from("Slow")),
            ("language-display", Value::from("Japanese")),
            ("language-system", Value::from("English")),
            ("contrast", Value::from("Light")),
            // Units — six independent fields that must all land, since a
            // one-byte shift would scramble exactly this run.
            ("altitude-distance", Value::from("ft/mi")),
            ("speed", Value::from("km/h")),
            ("temperature", Value::from("°C")),
            ("rainfall", Value::from("inch")),
            ("wind-speed", Value::from("knots")),
            // Right-hand column
            ("heterodyne", Value::from("Reverse")),
            ("charging-power-on", Value::Bool(false)),
            ("usb-power-input", Value::Bool(false)),
            ("battery-pack-confirmation", Value::Bool(false)),
            ("gps-time-correct", Value::from("Off")),
            // Signed, big-endian, and negative: UTC−09:30.
            ("utc-offset-minutes", Value::from(-570)),
            // Remote Mic — a sparse enum across two four-byte arrays.
            ("remote-mic-rx-a", Value::from("Down")),
            ("remote-mic-rx-b", Value::from("Call")),
            ("remote-mic-tx-a", Value::from("Vol Down")),
            ("remote-mic-tx-down", Value::from("T-Call")),
            // Screen capture
            ("screen-capture-power-key", Value::Bool(false)),
            ("screen-capture-filetype", Value::from("BMP")),
            // GPS tab
            ("gps-out-usb-port", Value::Bool(true)),
            ("sbas", Value::Bool(true)),
            ("glonass", Value::Bool(true)),
            ("gps-select", Value::from("Manual")),
            ("gps-pos-display-select", Value::from("SUB")),
            ("compass-direction", Value::from("North Up")),
            ("satellite-information-out", Value::from("GPS Only")),
            ("alarm-area-rx-mem", Value::from("Extended")),
            ("gps-logger", Value::Bool(true)),
            // Wx channels, the pair that lives outside the settings block.
            ("wx-channel-a", Value::from(8)),
            ("wx-channel-b", Value::from(4)),
        ];

        let mut wrong = Vec::new();
        for (key, want) in expect {
            match got.get(*key) {
                Some(v) if v == want => {}
                Some(v) => wrong.push(format!("  {key}: got {v}, expected {want}")),
                None => wrong.push(format!("  {key}: MISSING from the table")),
            }
        }
        assert!(
            wrong.is_empty(),
            "{} of {} settings decoded wrongly:\n{}",
            wrong.len(),
            expect.len(),
            wrong.join("\n")
        );
        eprintln!(
            "ok: {} settings decoded, {} spot-checked against the radio's screens",
            got.len(),
            expect.len()
        );
    }

    /// Decoding a real image and applying the result back must not move a
    /// single byte.
    ///
    /// This is the safety property that matters for a writer that patches the
    /// operator's own file: saving a profile you did not edit has to be a
    /// no-op. It is also the sharpest test of the table, because it fails if
    /// *any* field's encode is not the exact inverse of its decode — an enum
    /// whose label list has a gap, a text field padded differently, an integer
    /// written in the wrong byte order.
    ///
    ///     cargo test --lib icom_id52::settings -- --ignored --nocapture
    #[test]
    #[ignore = "needs a real .icf under scratchpad/id52/"]
    fn decode_then_apply_changes_nothing() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scratchpad/id52/auto23.icf"
        );
        let Ok(text) = std::fs::read_to_string(path) else {
            eprintln!("skipped: no capture at {path}");
            return;
        };
        let mut icf = super::super::icf::IcfFile::parse(&text).expect("a real ID-52 file parses");
        let before = icf.image().to_vec();
        let decoded = decode_settings(&before);
        let settings = decoded.as_object().expect("an object").clone();

        let applied = apply_settings(icf.image_mut(), &settings);
        assert_eq!(
            applied,
            settings.len(),
            "every decoded field should have been accepted on the way back"
        );

        let after = icf.image();
        let moved: Vec<usize> = (0..before.len()).filter(|&i| before[i] != after[i]).collect();
        assert!(
            moved.is_empty(),
            "{} bytes changed on a no-op save, first at {:06X}",
            moved.len(),
            moved.first().copied().unwrap_or(0)
        );
        eprintln!("ok: {applied} settings round-tripped, zero bytes moved");
    }

    /// Apply a single ad-hoc field, so a test can exercise an encoding without
    /// depending on which key the generated table happens to use for it.
    fn apply_one(image: &mut [u8], f: &SF, settings: &Map<String, Value>) {
        let table = [SF {
            key: f.key,
            byte: f.byte,
            kind: match &f.kind {
                SK::Uint { width } => SK::Uint { width: *width },
                SK::Int { width } => SK::Int { width: *width },
                SK::Bool => SK::Bool,
                SK::BoolBit { shift } => SK::BoolBit { shift: *shift },
                SK::Enum { width, labels } => SK::Enum {
                    width: *width,
                    labels,
                },
                SK::Text { len } => SK::Text { len: *len },
            },
        }];
        apply_table(image, &table, settings);
    }

    /// `apply_settings` against an arbitrary table rather than the generated
    /// one. Kept in the test module so production code has exactly one entry
    /// point.
    fn apply_table(image: &mut [u8], table: &[SF], settings: &Map<String, Value>) {
        for f in table {
            let Some(v) = settings.get(f.key) else {
                continue;
            };
            if decode_field(image, f) == *v {
                continue;
            }
            let at = f.byte as usize;
            match &f.kind {
                SK::Uint { width } => {
                    if let Some(n) = v.as_u64() {
                        write_uint(image, at, *width, n as u32)
                    }
                }
                SK::Int { width } => {
                    if let Some(n) = v.as_i64() {
                        write_int(image, at, *width, n)
                    }
                }
                SK::Bool => {
                    if let Some(b) = v.as_bool() {
                        image[at] = b as u8
                    }
                }
                SK::BoolBit { shift } => {
                    if let Some(b) = v.as_bool() {
                        let mask = 1u8 << shift;
                        image[at] = (image[at] & !mask) | if b { mask } else { 0 };
                    }
                }
                SK::Enum { width, labels } => {
                    if let Some(raw) = raw_for(labels, v) {
                        write_uint(image, at, *width, raw)
                    }
                }
                SK::Text { len } => {
                    if let Some(s) = v.as_str() {
                        write_text(image, at, *len as usize, s)
                    }
                }
            }
        }
    }
}
