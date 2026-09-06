//! The 600-series (APRS/TNC) settings, which live in the image and not in `MU`.
//!
//! ## Why this radio needs two transports for one settings form
//!
//! `MU` carries 42 menu parameters and **none of the 6xx group** — the feature
//! the radio is named for. Those live in six `0x480`-byte blocks at
//! `0x8100 + n * 0x480` behind `0M PROGRAM`: the live copy first, then PM1-5.
//! So one settings read is an `MU` exchange *and* a program-mode block read,
//! and one settings write is both again. [`super::settings`] joins them; this
//! module owns the image half.
//!
//! ## What is here, and what deliberately is not
//!
//! `scratchpad/kenwood_tmd710/APRS-MEASURED.md` grades every located field and
//! `gen_tmd710_aprs.py` emits this table and the profile schema from that one
//! sheet. **22 of the 66 individual settings in the 6xx/7xx range ship.** A row
//! ships only when its whole encoding is *anchored*: every index is measured on
//! the radio, is the factory default confirmed against the A manual, or is the
//! single remaining printed entry.
//!
//! ⚠ That bar exists because this radio's manual has printed a **short** option
//! list twice — menu 611's intervals (8 printed, at least 10 real) and menu
//! 625's display area (3 printed, the factory byte is `03`). "The rest of the
//! manual's list, in order" is therefore not an anchor here, and eleven located
//! fields are held back on exactly that ground. The sheet names the check that
//! settles each one.
//!
//! ⚠⚠ **SmartBeaconing is not on this radio.** The 7 bytes at `+0x3D7` match the
//! published SmartBeaconing defaults, but the word appears zero times in the
//! TM-D710**A** manual and there is no menu 630/631/632. They were graded
//! against the **G**'s manual, which this project used for two sessions before
//! noticing. No menu reaches them and they must never ship as settings.
//!
//! ## The one free check that validated the whole set
//!
//! The five PM copies are untouched factory defaults and the A manual states
//! the default of every menu, so each offset can be refuted at the desk. All
//! **19 checkable rows match**, including the five non-zero ones — `+0x00F`=`02`
//! =200 ms, `+0x011`=`01`=4800 bps, `+0x016`=`06`=6-char, `+0x1E0`=`0C`=100.0 Hz,
//! `+0x1E9`=`1C`=28 s — which are the identifying ones, since most defaults are
//! zero.
//!
//! ## The count, stated rather than implied
//!
//! "22 fields" is not a result. The reconciliation the `new-radio` skill asks
//! for, for this radio:
//!
//! | | menus | individual settings |
//! |---|---|---|
//! | the radio has | ~115 | |
//! | `MU` reaches | 42 | 35 shipped, 7 held (6 PF keys + p25, meanings unmeasured) |
//! | the 6xx/7xx image block holds | 34 | 66, of which **22 ship** |
//! | reached by neither | ~39 | the 1xx-5xx menus with no `MU` parameter |
//!
//! So the form is **57 fields**, and the 44 unshipped 6xx/7xx settings break
//! down as: 11 located but not anchored, 9 located but seen at a single value,
//! 6 text or record fields whose padding is unmeasured, and 18 unlocated. Every
//! one is a row in the sheet's `## Owed` table with the check that settles it.
//!
//! ## Writing
//!
//! A settings write is a **patch of differing runs**, never a whole-block write.
//! The block holds 44 bytes of status text, five 20-byte position records and
//! the operator's own call sign, none of which this form exposes; rewriting them
//! from a decoded-and-re-encoded block would put every one of them at risk of a
//! round-trip bug. Only bytes that actually change are sent, and the block is
//! read back afterwards — on this protocol an `0x06` is not a commit.

use serde_json::{json, Map, Value};

use super::image::{ProgramMode, APRS_BLOCK_LEN, APRS_LIVE};
use super::tone::TONES_DHZ;

/// One image-backed setting, as the generated table states it.
pub(crate) struct AF {
    pub key: &'static str,
    pub label: &'static str,
    /// The radio's own menu number, for the form's label.
    pub menu: &'static str,
    /// Byte offset from the live block base.
    pub off: usize,
    pub kind: AK,
}

impl AF {
    /// The form's label, with the menu number so a rejected value points at the
    /// menu to go and look at.
    pub(crate) fn display(&self) -> String {
        format!("{} (Menu {})", self.label, self.menu)
    }
}

pub(crate) enum AK {
    Bool,
    /// One bit of a shared mask byte. Menu 609's packet filter is six of these
    /// in `+0x167`, and **neither half of its packing is in the manual**: the
    /// screen's 2×3 grid is read down each column, and that list is packed
    /// MSB-first. Measured with `01`, `04` and `2A`; the factory `3F` is
    /// invariant under every rival ordering and could not have caught any of it.
    Bit { bit: u8 },
    Enum { labels: &'static [(u8, &'static str)] },
    Uint { min: u8, max: u8 },
    /// A 0-based index into the driver's own 42-tone CTCSS table — the same one
    /// the channel encoder uses, read rather than re-typed so the two cannot
    /// drift. Measured at `08` = 88.5 Hz and `0C` = 100.0 Hz.
    Ctcss,
}

include!("tmd710_aprs_table.rs");

/// `88.5 Hz` and friends, in table order.
pub(crate) fn ctcss_labels() -> Vec<String> {
    TONES_DHZ
        .iter()
        .map(|d| format!("{}.{} Hz", d / 10, d % 10))
        .collect()
}

/// Decode the live block into the profile form's shape.
pub(crate) fn decode(block: &[u8], out: &mut Map<String, Value>) {
    for f in TMD710_APRS_FIELDS {
        let Some(&b) = block.get(f.off) else { continue };
        let value = match &f.kind {
            AK::Bool => json!(b != 0),
            AK::Bit { bit } => json!(b & (1 << bit) != 0),
            AK::Uint { .. } => json!(b),
            AK::Ctcss => match ctcss_labels().get(b as usize) {
                Some(l) => json!(l),
                None => json!(b),
            },
            AK::Enum { labels } => match labels.iter().find(|(raw, _)| *raw == b) {
                Some((_, l)) => json!(l),
                // The same honest fallback the `MU` half uses: "your radio holds
                // something this table cannot name" is a measurement gap, not a
                // corrupt radio, and the number has to survive the round trip or
                // every later write fails.
                None => json!(b),
            },
        };
        out.insert(f.key.to_string(), value);
    }
}

/// One form value as the byte the radio stores.
fn encode_one(f: &AF, v: &Value) -> Result<u8, String> {
    Ok(match &f.kind {
        AK::Bool | AK::Bit { .. } => match v.as_bool() {
            Some(b) => u8::from(b),
            None => return Err(format!("{} expects true or false, got {v}", f.display())),
        },
        AK::Uint { min, max } => {
            let n = v
                .as_u64()
                .ok_or_else(|| format!("{} expects a number, got {v}", f.display()))?;
            if n < u64::from(*min) || n > u64::from(*max) {
                return Err(format!("{} is {n}, outside the radio's {min}..={max}", f.display()));
            }
            n as u8
        }
        AK::Ctcss => match v {
            Value::Number(n) => n
                .as_u64()
                .filter(|n| *n <= u64::from(u8::MAX))
                .ok_or_else(|| format!("{} cannot store {v}", f.display()))? as u8,
            _ => {
                let s = v
                    .as_str()
                    .ok_or_else(|| format!("{} expects a CTCSS tone, got {v}", f.display()))?;
                ctcss_labels()
                    .iter()
                    .position(|l| l == s)
                    .ok_or_else(|| format!("{} has no tone {s:?}", f.display()))? as u8
            }
        },
        AK::Enum { labels } => match v {
            Value::Number(n) => n
                .as_u64()
                .filter(|n| *n <= u64::from(u8::MAX))
                .ok_or_else(|| format!("{} cannot store {v}", f.display()))? as u8,
            _ => {
                let s = v
                    .as_str()
                    .ok_or_else(|| format!("{} expects one of its options, got {v}", f.display()))?;
                labels
                    .iter()
                    .find(|(_, l)| *l == s)
                    .map(|(raw, _)| *raw)
                    .ok_or_else(|| format!("{} has no option {s:?}", f.display()))?
            }
        },
    })
}

/// Patch the profile's fields over the block the radio currently holds.
///
/// ⚠ A **patch of the radio's own bytes**, exactly like the `MU` half. Every
/// byte this form does not expose — the call sign, five position records, five
/// 44-byte status texts, the whole Sky Command tail — goes back as it came,
/// because it is copied rather than re-encoded.
///
/// Returns the new block and the number of form fields whose value moved. A
/// masked byte counts once per field, which is what an operator changed.
pub(crate) fn patch(base: &[u8], settings: &Value) -> Result<(Vec<u8>, usize), String> {
    if base.len() != APRS_BLOCK_LEN {
        return Err(format!(
            "the APRS block is {APRS_BLOCK_LEN} bytes, got {}",
            base.len()
        ));
    }
    let mut out = base.to_vec();
    let mut changed = 0usize;
    for f in TMD710_APRS_FIELDS {
        let Some(v) = settings.get(f.key) else { continue };
        if v.is_null() {
            continue;
        }
        let encoded = encode_one(f, v)?;
        let before = out[f.off];
        out[f.off] = match &f.kind {
            AK::Bit { bit } => {
                let m = 1u8 << bit;
                if encoded != 0 { before | m } else { before & !m }
            }
            _ => encoded,
        };
        if out[f.off] != before {
            changed += 1;
        }
    }
    Ok((out, changed))
}

/// The spans that actually differ, coalesced.
///
/// Gaps shorter than [`STITCH`] are swallowed: two three-byte runs a byte apart
/// are one seven-byte write, and a write costs a whole request either way. Every
/// run is capped at 256, which is the largest block this protocol carries.
const STITCH: usize = 8;

pub(crate) fn differing_runs(a: &[u8], b: &[u8]) -> Vec<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for i in 0..a.len().min(b.len()) {
        if a[i] == b[i] {
            continue;
        }
        match runs.last_mut() {
            Some(last) if i - last.1 <= STITCH && i + 1 - last.0 <= 256 => last.1 = i + 1,
            _ => runs.push((i, i + 1)),
        }
    }
    runs
}

/// Read the live APRS block.
pub(crate) fn read_block(pm: &mut ProgramMode<'_>) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(APRS_BLOCK_LEN);
    while out.len() < APRS_BLOCK_LEN {
        let want = (APRS_BLOCK_LEN - out.len()).min(256);
        let addr = APRS_LIVE + out.len() as u16;
        let got = pm.read(addr, if want == 256 { 0 } else { want as u8 })?;
        if got.len() != want {
            return Err(format!(
                "reading the APRS block at 0x{addr:04X}: asked for {want} bytes, got {}",
                got.len()
            ));
        }
        out.extend_from_slice(&got);
    }
    Ok(out)
}

/// Write only what changed, then **read the block back**.
///
/// ⚠ The read-back is not belt and braces. This protocol answers a write with
/// `0x06` whether or not the radio kept it — an APRS block once answered `06`
/// four times running and never changed a byte. Returns the addresses written.
pub(crate) fn write_block_narrow(
    pm: &mut ProgramMode<'_>,
    base: &[u8],
    wanted: &[u8],
) -> Result<(Vec<String>, bool), String> {
    let runs = differing_runs(base, wanted);
    let mut written = Vec::new();
    for (start, end) in &runs {
        let addr = APRS_LIVE + *start as u16;
        pm.write(addr, &wanted[*start..*end])?;
        written.push(format!("0x{addr:04X}+{}", end - start));
    }
    let after = read_block(pm)?;
    Ok((written, after == wanted))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The live block out of `progfull-71022.bin`, as the radio served it.
    fn sample() -> Vec<u8> {
        let mut b = vec![0u8; APRS_BLOCK_LEN];
        // Only the bytes the table reads matter here; the rest stays zero.
        for (off, v) in [
            (0x00C, 0x00), (0x00D, 0x00), (0x00E, 0x00), (0x011, 0x01), (0x016, 0x06),
            (0x083, 0x01), (0x084, 0x01), (0x085, 0x00), (0x087, 0x00), (0x167, 0x3F),
            (0x16D, 0x02), (0x16F, 0x01), (0x170, 0x01), (0x1DF, 0x01), (0x1E0, 0x0C),
            (0x1E9, 0x1C), (0x362, 0x00),
        ] {
            b[off] = v;
        }
        b
    }

    /// ★ The pairing the `new-radio` skill requires, for the second transport:
    /// **one sheet, both halves.** A table entry with no form field is a setting
    /// nobody can reach; a form field with no table entry silently does nothing
    /// when saved. Both come out of `APRS-MEASURED.md` by one script, and this
    /// is what stops them drifting afterwards.
    #[test]
    fn the_aprs_table_and_the_profile_schema_describe_the_same_fields() {
        let schema: Vec<Value> =
            serde_json::from_str(crate::seed::TMD710_SETTINGS_SCHEMA).expect("schema parses");
        for f in TMD710_APRS_FIELDS {
            let e = schema
                .iter()
                .find(|e| e["key"] == f.key)
                .unwrap_or_else(|| panic!("{} has no form field", f.key));
            assert_eq!(e["label"], json!(f.display()), "{}", f.key);
            match &f.kind {
                AK::Bool | AK::Bit { .. } => assert_eq!(e["type"], "boolean", "{}", f.key),
                AK::Uint { min, max } => {
                    assert_eq!(e["type"], "integer", "{}", f.key);
                    assert_eq!(e["min"], json!(min), "{}", f.key);
                    assert_eq!(e["max"], json!(max), "{}", f.key);
                }
                AK::Ctcss => {
                    let opts: Vec<String> = e["options"]
                        .as_array()
                        .expect("options")
                        .iter()
                        .map(|o| o.as_str().expect("string").to_string())
                        .collect();
                    assert_eq!(opts, ctcss_labels(), "{} options disagree", f.key);
                }
                AK::Enum { labels } => {
                    let opts: Vec<&str> = e["options"]
                        .as_array()
                        .expect("options")
                        .iter()
                        .map(|o| o.as_str().expect("string"))
                        .collect();
                    let mine: Vec<&str> = labels.iter().map(|(_, l)| *l).collect();
                    assert_eq!(opts, mine, "{} options disagree", f.key);
                }
            }
        }
        // And the other direction: every `aprs-` field in the form is written by
        // a table entry.
        for e in &schema {
            let key = e["key"].as_str().expect("key");
            if !key.starts_with("aprs-") {
                continue;
            }
            assert!(
                TMD710_APRS_FIELDS.iter().any(|f| f.key == key),
                "the form offers {key:?}, which no table entry writes"
            );
        }
    }

    /// ⚠⚠ SmartBeaconing is not on this radio — no menu 630/631/632 exists on
    /// the A, and the bytes at `+0x3D7` that match the published defaults were
    /// graded against the **G**'s manual. This is the assertion that stops them
    /// being filled in from that table again.
    #[test]
    fn nothing_reaches_the_smartbeaconing_bytes() {
        for f in TMD710_APRS_FIELDS {
            assert!(
                !(0x3D7..0x3DE).contains(&f.off),
                "{} points into the SmartBeaconing bytes, which no menu on the A reaches",
                f.key
            );
        }
    }

    /// ★ The census, as an assertion rather than a sentence in a doc comment.
    ///
    /// The `new-radio` skill's gate is a **stated count** — "N of the radio's
    /// M", with the M-N named — because "35 fields" was what this radio shipped
    /// while missing its headline feature. Pinning the numbers means growing the
    /// table forces someone to restate them, which is the only way a count in
    /// prose stays true.
    #[test]
    fn the_census_is_stated_rather_than_implied() {
        assert_eq!(
            TMD710_APRS_FIELDS.len(),
            22,
            "22 of the 66 individual settings in the radio's 6xx/7xx menus. If this moved,              update the census table in the module doc and the ## Owed rows in              scratchpad/kenwood_tmd710/APRS-MEASURED.md — a count nobody restates goes stale."
        );
        let schema: Vec<Value> =
            serde_json::from_str(crate::seed::TMD710_SETTINGS_SCHEMA).expect("schema parses");
        assert_eq!(
            schema.len(),
            22 + 35,
            "the profile form is both transports: 35 MU fields and 22 image fields"
        );
        // Nine menu numbers are represented. The radio has 34 in this range.
        let mut menus: Vec<&str> = TMD710_APRS_FIELDS.iter().map(|f| f.menu).collect();
        menus.sort_unstable();
        menus.dedup();
        assert_eq!(menus, ["601", "602", "603", "606", "607", "609", "611", "614", "617", "626"]);
    }

    /// Keys are what a saved profile stores, and a byte may be shared only when
    /// each field owns a distinct bit of it.
    #[test]
    fn keys_are_unique_and_no_two_fields_own_one_byte() {
        let mut keys: Vec<&str> = TMD710_APRS_FIELDS.iter().map(|f| f.key).collect();
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate settings key");

        let mut slots: Vec<(usize, i16)> = TMD710_APRS_FIELDS
            .iter()
            .map(|f| match f.kind {
                AK::Bit { bit } => (f.off, i16::from(bit)),
                _ => (f.off, -1),
            })
            .collect();
        slots.sort_unstable();
        slots.dedup();
        assert_eq!(slots.len(), n, "two fields claim the same byte");
        assert!(
            TMD710_APRS_FIELDS.iter().all(|f| f.off < APRS_BLOCK_LEN),
            "a field points past the {APRS_BLOCK_LEN}-byte block"
        );
    }

    /// The radio's own bytes decode to what its screen was showing.
    #[test]
    fn the_real_block_decodes() {
        let mut m = Map::new();
        decode(&sample(), &mut m);
        let v = Value::Object(m);
        assert_eq!(v["aprs-gps-baud"], json!("4800 bps"));
        assert_eq!(v["aprs-waypoint-name"], json!("6-char"));
        assert_eq!(v["aprs-beacon-method"], json!("Auto"), "+0x16D was 02");
        assert_eq!(v["aprs-voice-alert"], json!(true), "Tim read VOICE ALERT as On");
        assert_eq!(v["aprs-voice-alert-ctcss"], json!("100.0 Hz"), "+0x1E0 was 0C");
        assert_eq!(v["aprs-ui-check-time"], json!(28), "+0x1E9 is literal seconds");
        assert_eq!(v["aprs-temperature-unit"], json!("Fahrenheit"));
        // `3F` is all six filter bits.
        for k in ["weather", "mobile", "navitra", "digi", "object", "others"] {
            assert_eq!(v[format!("aprs-filter-{k}")], json!(true), "{k}");
        }
    }

    /// ★ The packet filter as it was actually measured: `04` marked Digi (which
    /// killed the "printed list reversed" reading, that predicts Object), and
    /// `2A` marked Weather, Navitra and Object. `3F` is invariant under every
    /// rival ordering and proves nothing.
    #[test]
    fn the_packet_filter_bits_are_the_ones_measured_on_the_radio() {
        for (mask, on) in [
            (0x01u8, vec!["others"]),
            (0x04, vec!["digi"]),
            (0x2A, vec!["weather", "navitra", "object"]),
        ] {
            let mut block = sample();
            block[0x167] = mask;
            let mut m = Map::new();
            decode(&block, &mut m);
            for k in ["weather", "mobile", "navitra", "digi", "object", "others"] {
                assert_eq!(
                    m[&format!("aprs-filter-{k}")],
                    json!(on.contains(&k)),
                    "mask {mask:02X}, {k}"
                );
            }
        }
    }

    /// ★ A patch touches the bytes it was asked for and **nothing else** — the
    /// call sign, the position records and the status texts are not this form's
    /// to rewrite.
    #[test]
    fn patching_leaves_every_byte_the_form_does_not_expose_alone() {
        let mut base = sample();
        base[0x000..0x00A].copy_from_slice(b"W0ABC-9\0\0\0"); // call sign
        base[0x089] = b'H'; // status text 1
        base[0x474] = 0x08; // Sky Command tone

        let (out, changed) = patch(&base, &json!({ "aprs-temperature-unit": "Celsius" })).unwrap();
        assert_eq!(changed, 1);
        assert_eq!(out[0x362], 0x01);
        assert_eq!(differing_runs(&base, &out), vec![(0x362, 0x363)]);
        assert_eq!(&out[0x000..0x00A], &base[0x000..0x00A]);
        assert_eq!(out[0x089], b'H');
        assert_eq!(out[0x474], 0x08);
    }

    /// Six form fields share one byte, so clearing one must leave the other five.
    #[test]
    fn a_masked_field_changes_only_its_own_bit() {
        let base = sample();
        let (out, changed) = patch(&base, &json!({ "aprs-filter-digi": false })).unwrap();
        assert_eq!(changed, 1);
        assert_eq!(out[0x167], 0x3B, "only bit 2 should have cleared");
    }

    /// A value round-trips through the form's labels and back to the same byte.
    #[test]
    fn every_field_round_trips_through_its_labels() {
        let base = sample();
        let mut m = Map::new();
        decode(&base, &mut m);
        let (out, changed) = patch(&base, &Value::Object(m)).unwrap();
        assert_eq!(changed, 0, "decoding and re-encoding must move nothing");
        assert_eq!(out, base);
    }

    /// An unlabelled stored value survives the round trip as a number. Refuse it
    /// and every later write fails, which is how the TH-D72 became unwritable.
    #[test]
    fn an_unlabelled_value_round_trips_as_a_number() {
        let mut base = sample();
        base[0x087] = 64; // no such position comment
        let mut m = Map::new();
        decode(&base, &mut m);
        assert_eq!(m["aprs-position-comment"], json!(64));
        let (out, _) = patch(&base, &Value::Object(m)).unwrap();
        assert_eq!(out[0x087], 64);
    }

    #[test]
    fn a_value_outside_the_measured_range_is_refused() {
        let f = TMD710_APRS_FIELDS
            .iter()
            .find(|f| f.key == "aprs-ui-check-time")
            .unwrap();
        let err = encode_one(f, &json!(251)).unwrap_err();
        assert!(err.contains("0..=250") && err.contains("Menu 617"), "{err}");
        let band = TMD710_APRS_FIELDS
            .iter()
            .find(|f| f.key == "aprs-data-band")
            .unwrap();
        assert!(encode_one(band, &json!("C band")).is_err());
    }

    /// Runs are coalesced across small gaps and capped at one block.
    #[test]
    fn differing_runs_coalesce_and_cap() {
        let a = vec![0u8; 600];
        let mut b = a.clone();
        b[10] = 1;
        b[14] = 1; // 3 bytes clear -> stitched
        b[40] = 1; // 25 clear -> its own run
        assert_eq!(differing_runs(&a, &b), vec![(10, 15), (40, 41)]);

        let long: Vec<u8> = (0..600).map(|_| 1u8).collect();
        let runs = differing_runs(&a, &long);
        assert!(runs.iter().all(|(s, e)| e - s <= 256), "{runs:?}");
        assert_eq!(runs.iter().map(|(s, e)| e - s).sum::<usize>(), 600);
    }
}
