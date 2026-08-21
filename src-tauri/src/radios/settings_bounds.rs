//! One range check for every profile settings value on its way to a radio.
//!
//! A settings schema declares `min`/`max` on its integer fields, and until this
//! module those numbers were decoration: the frontend rendered them as HTML
//! attributes (which flag an out-of-range value but do not prevent one) and
//! every encoder cast whatever arrived straight down to a byte. A UV-5R
//! backlight timeout of 300 became `300 as u8` — byte 44 — on the radio (#87).
//!
//! The cast is per-driver and there is one in each of them, so the check sits
//! in front of all of them instead. The three places where a profile's values
//! meet its model's schema run it first:
//!
//! - a settings write over the cable (`write_radio_settings`),
//! - a codeplug program that carries settings (the image-programmer path),
//! - a card export that patches settings into the radio's own file.
//!
//! ## Why it drops the value instead of refusing the write
//!
//! Refusing looked right until it was run against a real profile. The dev
//! UV-5R profile — read off the radio, never hand-edited — holds `0` for all
//! four band limits, whose schema says `min: 1`. That radio genuinely stores
//! zeros there, so a hard refusal would have blocked every settings write and
//! every codeplug program for that profile, and the operator's only way out
//! would have been to invent values the radio never had. A schema's range is
//! this app's claim about a radio, and three of the four radios with bounded
//! fields have never had that claim tested against what they actually store.
//!
//! So an out-of-range value is dropped, not written and not corrected: the
//! field keeps whatever the radio already has, and the caller gets a note
//! naming it. Nothing is silently truncated, and nothing is silently blocked.
//! The profile editor is where such a value is *seen* — it marks the field —
//! and it refuses to store an out-of-range value the operator typed there.
//!
//! Only integer fields are checked. A `select` deliberately keeps a value its
//! option list does not contain — that is how a value read off a radio this app
//! cannot name survives a round trip — and text fields are cut to their field
//! width by the encoder that knows the width.

use serde_json::Value;

/// Remove every value the schema says the radio cannot take, returning one note
/// per field removed (empty when there is nothing to say).
///
/// A schema that will not parse is not this function's business — the driver
/// that needs it reports that itself — so it changes nothing.
pub(crate) fn strip_out_of_range(schema_json: &str, settings: &mut Value) -> Vec<String> {
    let Ok(schema) = serde_json::from_str::<Value>(schema_json) else {
        return Vec::new();
    };
    let (Some(fields), Some(values)) = (schema.as_array(), settings.as_object_mut()) else {
        return Vec::new();
    };

    let mut notes: Vec<String> = Vec::new();
    let mut drop_keys: Vec<String> = Vec::new();
    for field in fields {
        if field.get("type").and_then(Value::as_str) != Some("integer") {
            continue;
        }
        let Some(key) = field.get("key").and_then(Value::as_str) else {
            continue;
        };
        // Absent, or blank — a card radio's profile starts with every field
        // unset on purpose, and an unset field is written by nobody.
        let Some(Value::Number(n)) = values.get(key) else {
            continue;
        };
        let label = field.get("label").and_then(Value::as_str).unwrap_or(key);

        // Whole numbers only: every encoder resolves an integer field through
        // `as_i64`, so 3.5 is not "3.5 rounded", it is a field silently left at
        // whatever the radio already held. Say so rather than let it look
        // written.
        let Some(v) = n.as_i64() else {
            notes.push(format!("{label} is {n}, which is not a whole number"));
            drop_keys.push(key.to_string());
            continue;
        };
        let min = field.get("min").and_then(Value::as_i64);
        let max = field.get("max").and_then(Value::as_i64);
        let outside = match (min, max) {
            (Some(lo), Some(hi)) if v < lo || v > hi => Some(format!("outside {lo}–{hi}")),
            (Some(lo), None) if v < lo => Some(format!("below {lo}")),
            (None, Some(hi)) if v > hi => Some(format!("above {hi}")),
            _ => None,
        };
        if let Some(how) = outside {
            notes.push(format!("{label} is {v}, {how}"));
            drop_keys.push(key.to_string());
        }
    }

    for key in drop_keys {
        values.remove(&key);
    }
    // Stable order regardless of the settings object's key order, so the same
    // profile always reports the same way.
    notes.sort();
    notes
}

/// The notes as one line for a report that has room for a sentence rather than
/// a list. `None` when nothing was dropped.
pub(crate) fn note_line(notes: &[String]) -> Option<String> {
    if notes.is_empty() {
        return None;
    }
    Some(format!(
        "{} setting{} not written because {} outside the range this radio accepts: {}. \
         The radio keeps its own value for {}.",
        notes.len(),
        if notes.len() == 1 { "" } else { "s" },
        if notes.len() == 1 { "it is" } else { "they are" },
        notes.join("; "),
        if notes.len() == 1 { "it" } else { "them" },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SCHEMA: &str = r#"[
        {"key": "abr", "label": "Backlight Timeout (s)", "type": "integer", "min": 0, "max": 24},
        {"key": "squelch", "label": "Squelch", "type": "integer", "min": 0, "max": 9},
        {"key": "ssid", "label": "My SSID", "type": "integer"},
        {"key": "floor", "label": "Floor", "type": "integer", "min": 1},
        {"key": "save", "label": "Battery Saver", "type": "select", "options": ["Off", "1:1"]},
        {"key": "callsign", "label": "Call sign", "type": "text", "max_length": 6}
    ]"#;

    fn strip(json: Value) -> (Value, Vec<String>) {
        let mut v = json;
        let notes = strip_out_of_range(SCHEMA, &mut v);
        (v, notes)
    }

    #[test]
    fn values_inside_their_range_are_left_alone() {
        let (v, notes) = strip(json!({"abr": 24, "squelch": 0, "ssid": 9999, "floor": 1}));
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(v, json!({"abr": 24, "squelch": 0, "ssid": 9999, "floor": 1}));
    }

    /// The failure in #87, exactly as reported: 300 in a 0–24 field reached the
    /// UV-5R encoder, which cast it to byte 44. Now it reaches nothing.
    #[test]
    fn the_backlight_timeout_that_became_byte_44_is_dropped() {
        let (v, notes) = strip(json!({"abr": 300, "squelch": 4}));
        assert_eq!(notes, ["Backlight Timeout (s) is 300, outside 0–24"]);
        assert_eq!(v, json!({"squelch": 4}), "only the bad field is dropped");
    }

    #[test]
    fn a_negative_value_goes_the_same_way() {
        let (_, notes) = strip(json!({"squelch": -1}));
        assert_eq!(notes, ["Squelch is -1, outside 0–9"]);
    }

    /// One-sided bounds are checked on the side they have and left alone on the
    /// side they do not — the FT5D schema has fields with neither.
    #[test]
    fn a_one_sided_bound_checks_only_that_side() {
        assert_eq!(strip(json!({"floor": 0})).1.len(), 1);
        assert!(strip(json!({"floor": 1_000_000})).1.is_empty());
        assert!(strip(json!({"ssid": -400})).1.is_empty());
    }

    /// A `select` holding a value its option list does not name is how a
    /// setting read off a radio survives a round trip, and a card radio's
    /// profile starts with every field blank. Neither is this check's business.
    #[test]
    fn selects_blanks_and_missing_keys_pass_through_untouched() {
        let before = json!({"save": "1:9", "abr": "", "callsign": "WW8LWW8L", "unknown": 900});
        let (after, notes) = strip(before.clone());
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(after, before);
    }

    #[test]
    fn a_fractional_integer_is_named_rather_than_silently_dropped() {
        let (v, notes) = strip(json!({"squelch": 3.5}));
        assert_eq!(notes, ["Squelch is 3.5, which is not a whole number"]);
        assert_eq!(v, json!({}));
    }

    #[test]
    fn every_offending_field_is_named_in_a_stable_order() {
        let (_, notes) = strip(json!({"squelch": 99, "abr": 300}));
        assert_eq!(
            notes,
            [
                "Backlight Timeout (s) is 300, outside 0–24",
                "Squelch is 99, outside 0–9"
            ]
        );
    }

    /// The dev UV-5R profile, read off the radio and never hand-edited, holds 0
    /// for four band limits whose schema says `min: 1`. Dropping those four
    /// leaves a perfectly writable profile; refusing the write would have left
    /// the operator inventing limits the radio never had.
    #[test]
    fn a_profile_read_off_a_radio_stays_writable() {
        let schema = r#"[
            {"key": "limits.vhf.lower", "label": "VHF Lower Limit (MHz)", "type": "integer", "min": 1, "max": 1000},
            {"key": "squelch", "label": "Carrier Squelch Level", "type": "integer", "min": 0, "max": 9}
        ]"#;
        let mut v = json!({"limits.vhf.lower": 0, "squelch": 3});
        let notes = strip_out_of_range(schema, &mut v);
        assert_eq!(notes.len(), 1);
        assert_eq!(v, json!({"squelch": 3}));
    }

    /// A schema this app cannot parse is the driver's problem to report, not a
    /// reason to drop anything here.
    #[test]
    fn an_unreadable_schema_changes_nothing() {
        let mut v = json!({"abr": 300});
        assert!(strip_out_of_range("not json", &mut v).is_empty());
        assert!(strip_out_of_range("{}", &mut v).is_empty());
        assert_eq!(v, json!({"abr": 300}));
    }

    #[test]
    fn the_note_line_reads_as_a_sentence_and_is_absent_when_there_is_nothing_to_say() {
        assert_eq!(note_line(&[]), None);
        let one = note_line(&["Squelch is 99, outside 0–9".into()]).unwrap();
        assert!(one.starts_with("1 setting not written because it is outside"), "{one}");
        let two = note_line(&["A is 1, above 0".into(), "B is 2, above 0".into()]).unwrap();
        assert!(two.starts_with("2 settings not written because they are outside"), "{two}");
        assert!(two.contains("A is 1, above 0; B is 2, above 0"), "{two}");
    }
}
