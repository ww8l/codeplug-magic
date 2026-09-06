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
//! `gen_tmd710_image.py` emits this table and the profile schema from that one
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
//! | the 6xx/7xx image block holds | 34 | 66, of which **31 ship** |
//! | reached by neither | ~39 | the 1xx-5xx menus with no `MU` parameter |
//!
//! So the form is **66 fields**, and the 35 unshipped 6xx/7xx settings each have
//! a row in the sheet's `## Owed` table naming the check that settles it: one
//! (624 RX BEEP) has contradictory readings, nine are located but seen at a
//! single value, six are text or record fields whose padding is unmeasured, and
//! the rest are unlocated.
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

/// The image regions this form reads and writes.
///
/// ⚠ Two, not one. The 600-series settings are in the APRS block, but menu 500's
/// POWER ON MESSAGE is in the **PM0 config block** at `0x0200` — a different
/// region entirely, and the reason this module is no longer called `aprs`.
/// A window is read whole, patched, and written back only where it differs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum W {
    /// PM0 config, `0x0200`. ⚠ Holds the five volatile operating-state bytes.
    Config,
    /// The live APRS/TNC block, `0x8100`.
    Aprs,
}

/// `0x0200` block 0; PM1-5 follow at a `0x200` stride, and this form never
/// touches those — they are the operator's saved profiles.
const CONFIG_BASE: u16 = 0x0200;
const CONFIG_LEN: usize = 0x200;

impl W {
    pub(crate) fn base(self) -> u16 {
        match self {
            W::Config => CONFIG_BASE,
            W::Aprs => APRS_LIVE,
        }
    }
    pub(crate) fn len(self) -> usize {
        match self {
            W::Config => CONFIG_LEN,
            W::Aprs => APRS_BLOCK_LEN,
        }
    }
    pub(crate) const ALL: [W; 2] = [W::Config, W::Aprs];
}

/// The windows as read off the radio, in [`W::ALL`] order.
///
/// Carried as a list of `(window, bytes)` rather than a struct with a named
/// field per window so that adding a third region is a table entry, not a new
/// type — the config window was itself a late addition.
pub(crate) type Windows = Vec<(W, Vec<u8>)>;

/// One image-backed setting, as the generated table states it.
pub(crate) struct AF {
    pub key: &'static str,
    pub label: &'static str,
    /// The radio's own menu number, for the form's label.
    pub menu: &'static str,
    /// Which image window the offset is in.
    pub win: W,
    /// Byte offset from that window's base.
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
    /// A fixed-width text field. `bytes` is the space it occupies, `chars` the
    /// most the radio will show, and `pad` the byte the RADIO ITSELF writes
    /// after the text — measured, and ⚠ **not the same for both text fields on
    /// this radio**: the APRS call sign pads with `00` and the power-on message
    /// with `FF`. Assuming one from the other would have been wrong.
    Text { bytes: usize, chars: usize, pad: u8 },
    /// A 0-based index into the driver's own 42-tone CTCSS table — the same one
    /// the channel encoder uses, read rather than re-typed so the two cannot
    /// drift. Measured at `08` = 88.5 Hz and `0C` = 100.0 Hz.
    Ctcss,
}

include!("tmd710_image_table.rs");

/// `88.5 Hz` and friends, in table order.
pub(crate) fn ctcss_labels() -> Vec<String> {
    TONES_DHZ
        .iter()
        .map(|d| format!("{}.{} Hz", d / 10, d % 10))
        .collect()
}

/// Read one field's bytes out of the window it lives in.
fn bytes_of<'a>(wins: &'a [(W, Vec<u8>)], f: &AF, n: usize) -> Option<&'a [u8]> {
    let (_, buf) = wins.iter().find(|(w, _)| *w == f.win)?;
    buf.get(f.off..f.off + n)
}

/// Decode the windows into the profile form's shape.
pub(crate) fn decode(wins: &[(W, Vec<u8>)], out: &mut Map<String, Value>) {
    for f in TMD710_IMAGE_FIELDS {
        if let AK::Text { bytes, pad, .. } = f.kind {
            let Some(raw) = bytes_of(wins, f, bytes) else { continue };
            // Everything up to the first pad byte. The radio writes the pad
            // itself, so trimming it is reading, not cleaning up.
            let end = raw.iter().position(|b| *b == pad).unwrap_or(raw.len());
            let text: String = raw[..end].iter().map(|b| *b as char).collect();
            out.insert(f.key.to_string(), json!(text));
            continue;
        }
        let Some(&b) = bytes_of(wins, f, 1).map(|s| &s[0]) else { continue };
        let value = match &f.kind {
            // Handled above; the `continue` there is what makes this arm dead.
            AK::Text { .. } => unreachable!("text decodes before this match"),
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

/// One text value as the bytes the radio stores, padded as the radio pads.
fn encode_text(f: &AF, v: &Value) -> Result<Vec<u8>, String> {
    let AK::Text { bytes, chars, pad } = f.kind else {
        unreachable!("encode_text on a non-text field")
    };
    let s = v
        .as_str()
        .ok_or_else(|| format!("{} expects text, got {v}", f.display()))?;
    if s.chars().count() > chars {
        return Err(format!(
            "{} is {} characters; the radio holds {chars}",
            f.display(),
            s.chars().count()
        ));
    }
    // ⚠ Refused rather than silently dropped. A call sign quietly stripped of a
    // character is worse than a rejected write: it goes on the air.
    if let Some(bad) = s.chars().find(|c| !(' '..='~').contains(c)) {
        return Err(format!("{} cannot store {bad:?}", f.display()));
    }
    let mut out = vec![pad; bytes];
    for (i, c) in s.chars().enumerate() {
        out[i] = c as u8;
    }
    Ok(out)
}

/// One form value as the byte the radio stores.
fn encode_one(f: &AF, v: &Value) -> Result<u8, String> {
    Ok(match &f.kind {
        AK::Text { .. } => unreachable!("text goes through encode_text"),
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

/// Patch the profile's fields over the windows the radio currently holds.
///
/// ⚠ A **patch of the radio's own bytes**, exactly like the `MU` half. Every
/// byte this form does not expose — five position records, five 44-byte status
/// texts, the whole Sky Command tail, and in the config window the operator's
/// VFO settings and the five volatile operating-state bytes — goes back as it
/// came, because it is copied rather than re-encoded.
///
/// Returns the patched windows and the number of form fields whose value moved.
/// A masked byte counts once per field, which is what an operator changed.
pub(crate) fn patch(base: &[(W, Vec<u8>)], settings: &Value) -> Result<(Windows, usize), String> {
    for (w, buf) in base {
        if buf.len() != w.len() {
            return Err(format!("{w:?} is {} bytes, got {}", w.len(), buf.len()));
        }
    }
    let mut out: Vec<(W, Vec<u8>)> = base.to_vec();
    let mut changed = 0usize;
    for f in TMD710_IMAGE_FIELDS {
        let Some(v) = settings.get(f.key) else { continue };
        if v.is_null() {
            continue;
        }
        let Some((_, buf)) = out.iter_mut().find(|(w, _)| *w == f.win) else { continue };

        if let AK::Text { bytes, .. } = f.kind {
            let encoded = encode_text(f, v)?;
            if buf[f.off..f.off + bytes] != encoded[..] {
                buf[f.off..f.off + bytes].copy_from_slice(&encoded);
                changed += 1;
            }
            continue;
        }

        let encoded = encode_one(f, v)?;
        let before = buf[f.off];
        buf[f.off] = match &f.kind {
            AK::Bit { bit } => {
                let m = 1u8 << bit;
                if encoded != 0 { before | m } else { before & !m }
            }
            _ => encoded,
        };
        if buf[f.off] != before {
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

/// Read one window.
pub(crate) fn read_window(pm: &mut ProgramMode<'_>, w: W) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(w.len());
    while out.len() < w.len() {
        let want = (w.len() - out.len()).min(256);
        let addr = w.base() + out.len() as u16;
        let got = pm.read(addr, if want == 256 { 0 } else { want as u8 })?;
        if got.len() != want {
            return Err(format!(
                "reading {w:?} at 0x{addr:04X}: asked for {want} bytes, got {}",
                got.len()
            ));
        }
        out.extend_from_slice(&got);
    }
    Ok(out)
}

/// Every window this form covers.
pub(crate) fn read_all(pm: &mut ProgramMode<'_>) -> Result<Windows, String> {
    W::ALL.iter().map(|w| Ok((*w, read_window(pm, *w)?))).collect()
}

/// Write only what changed, then **read the written spans back**.
///
/// ⚠ The read-back is not belt and braces. This protocol answers a write with
/// `0x06` whether or not the radio kept it — an APRS block once answered `06`
/// four times running and never changed a byte.
///
/// ⚠⚠ Only the spans that were **written** are compared, not the whole window.
/// The config window holds five operating-state bytes that drift as the operator
/// walks menus, so a whole-window compare would report a perfectly good write as
/// unverified whenever someone touched the front panel mid-write.
pub(crate) fn write_narrow(
    pm: &mut ProgramMode<'_>,
    base: &[(W, Vec<u8>)],
    wanted: &[(W, Vec<u8>)],
) -> Result<(Vec<String>, bool), String> {
    let mut written = Vec::new();
    let mut verified = true;
    for (w, want) in wanted {
        let Some((_, have)) = base.iter().find(|(bw, _)| bw == w) else { continue };
        for (start, end) in differing_runs(have, want) {
            let addr = w.base() + start as u16;
            pm.write(addr, &want[start..end])?;
            written.push(format!("0x{addr:04X}+{}", end - start));

            let back = pm.read(addr, if end - start == 256 { 0 } else { (end - start) as u8 })?;
            if back != want[start..end] {
                verified = false;
            }
        }
    }
    Ok((written, verified))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two windows as the radio served them, from `progfull-71022.bin`.
    fn sample() -> Vec<(W, Vec<u8>)> {
        let mut cfg = vec![0u8; CONFIG_LEN];
        cfg[0x0E0..0x0E8].copy_from_slice(b"WW8L\xff\xff\xff\xff");

        let mut aprs = vec![0u8; APRS_BLOCK_LEN];
        aprs[0x000..0x00A].copy_from_slice(b"WW8L-1\0\0\0\0");
        for (off, v) in [
            (0x00C, 0x00), (0x00D, 0x00), (0x00E, 0x00), (0x011, 0x01), (0x016, 0x06),
            (0x083, 0x01), (0x084, 0x01), (0x085, 0x00), (0x087, 0x00), (0x167, 0x3F),
            (0x16D, 0x02), (0x16F, 0x01), (0x170, 0x01), (0x1DF, 0x01), (0x1E0, 0x0C),
            (0x1E9, 0x1C), (0x362, 0x00),
        ] {
            aprs[off] = v;
        }
        vec![(W::Config, cfg), (W::Aprs, aprs)]
    }

    fn decoded() -> Map<String, Value> {
        let mut m = Map::new();
        decode(&sample(), &mut m);
        m
    }

    /// ★ The pairing the `new-radio` skill requires, for the image transport:
    /// **one sheet, both halves.** A table entry with no form field is a setting
    /// nobody can reach; a form field with no table entry silently does nothing
    /// when saved. Both come out of `APRS-MEASURED.md` by one script, and this
    /// is what stops them drifting afterwards.
    #[test]
    fn the_image_table_and_the_profile_schema_describe_the_same_fields() {
        let schema: Vec<Value> =
            serde_json::from_str(crate::seed::TMD710_SETTINGS_SCHEMA).expect("schema parses");
        for f in TMD710_IMAGE_FIELDS {
            let e = schema
                .iter()
                .find(|e| e["key"] == f.key)
                .unwrap_or_else(|| panic!("{} has no form field", f.key));
            assert_eq!(e["label"], json!(f.display()), "{}", f.key);
            match &f.kind {
                AK::Bool | AK::Bit { .. } => assert_eq!(e["type"], "boolean", "{}", f.key),
                AK::Text { chars, .. } => {
                    assert_eq!(e["type"], "text", "{}", f.key);
                    assert_eq!(e["max_length"], json!(chars), "{}", f.key);
                }
                AK::Uint { min, max } => {
                    assert_eq!(e["type"], "integer", "{}", f.key);
                    assert_eq!(e["min"], json!(min), "{}", f.key);
                    assert_eq!(e["max"], json!(max), "{}", f.key);
                }
                AK::Ctcss => {
                    assert_eq!(e["type"], "select", "{}", f.key);
                    let opts: Vec<String> = e["options"]
                        .as_array()
                        .expect("options")
                        .iter()
                        .map(|o| o.as_str().expect("string").to_string())
                        .collect();
                    assert_eq!(opts, ctcss_labels(), "{} options disagree", f.key);
                }
                AK::Enum { labels } => {
                    assert_eq!(e["type"], "select", "{}", f.key);
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
        let mine: Vec<&str> = TMD710_IMAGE_FIELDS.iter().map(|f| f.key).collect();
        for e in &schema {
            if e["type"] == "section" {
                continue;
            }
            let key = e["key"].as_str().expect("key");
            if key.starts_with("aprs-") || key == "power-on-message" {
                assert!(
                    mine.contains(&key),
                    "the form offers {key:?}, which no table entry writes"
                );
            }
        }
    }

    /// ★ The census, as an assertion rather than a sentence in a doc comment.
    #[test]
    fn the_census_is_stated_rather_than_implied() {
        assert_eq!(
            TMD710_IMAGE_FIELDS.len(),
            33,
            "32 of the 66 individual settings in the radio's 6xx/7xx menus, plus menu \
             500's power-on message from the config window. If this moved, update the \
             census in the module doc and the ## Owed rows in APRS-MEASURED.md."
        );
        let schema: Vec<Value> =
            serde_json::from_str(crate::seed::TMD710_SETTINGS_SCHEMA).expect("schema parses");
        let fields = schema.iter().filter(|e| e["type"] != "section").count();
        assert_eq!(fields, 33 + 35, "the form is both transports");
        // ★ And it is GROUPED, like every other radio here. The TM-D710 was the
        // only one shipping a flat list, which is what made 68 controls
        // unreadable.
        let sections = schema.len() - fields;
        assert!(sections >= 10, "only {sections} section headings for {fields} fields");
    }

    /// ⚠⚠ SmartBeaconing is not on this radio — no menu 630/631/632 exists on
    /// the A, and the bytes at `+0x3D7` that match the published defaults were
    /// graded against the **G**'s manual.
    #[test]
    fn nothing_reaches_the_smartbeaconing_bytes() {
        for f in TMD710_IMAGE_FIELDS {
            assert!(
                !(f.win == W::Aprs && (0x3D7..0x3DE).contains(&f.off)),
                "{} points into the SmartBeaconing bytes, which no menu on the A reaches",
                f.key
            );
        }
    }

    /// ⚠ Nothing may reach the five operating-state bytes in the config window.
    /// They drift as the operator walks menus and are not settings; writing one
    /// would fight the radio, and reading one into a profile would make every
    /// saved profile differ from every other for no reason.
    #[test]
    fn nothing_reaches_the_volatile_operating_state() {
        for f in TMD710_IMAGE_FIELDS {
            if f.win != W::Config {
                continue;
            }
            let n = match f.kind {
                AK::Text { bytes, .. } => bytes,
                _ => 1,
            };
            for off in f.off..f.off + n {
                let addr = CONFIG_BASE as usize + off;
                assert!(
                    ![0x0216, 0x0222, 0x0224, 0x0228, 0x022E].contains(&addr),
                    "{} covers 0x{addr:04X}, which is volatile operating state",
                    f.key
                );
            }
        }
    }

    /// Keys are what a saved profile stores, and a byte may be shared only when
    /// each field owns a distinct bit of it.
    #[test]
    fn keys_are_unique_and_no_two_fields_own_one_byte() {
        let mut keys: Vec<&str> = TMD710_IMAGE_FIELDS.iter().map(|f| f.key).collect();
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate settings key");

        let mut slots: Vec<(u16, usize, i16)> = TMD710_IMAGE_FIELDS
            .iter()
            .map(|f| match f.kind {
                AK::Bit { bit } => (f.win.base(), f.off, i16::from(bit)),
                _ => (f.win.base(), f.off, -1),
            })
            .collect();
        slots.sort_unstable();
        slots.dedup();
        assert_eq!(slots.len(), n, "two fields claim the same byte");
        assert!(
            TMD710_IMAGE_FIELDS.iter().all(|f| f.off < f.win.len()),
            "a field points past the end of its window"
        );
    }

    /// The radio's own bytes decode to what its screen was showing.
    #[test]
    fn the_real_windows_decode() {
        let v = Value::Object(decoded());
        assert_eq!(v["aprs-gps-baud"], json!("4800 bps"));
        assert_eq!(v["aprs-beacon-method"], json!("Auto"), "+0x16D was 02");
        assert_eq!(v["aprs-voice-alert"], json!(true), "Tim read VOICE ALERT as On");
        assert_eq!(v["aprs-voice-alert-ctcss"], json!("100.0 Hz"), "+0x1E0 was 0C");
        assert_eq!(v["aprs-ui-check-time"], json!(28), "+0x1E9 is literal seconds");
        for k in ["weather", "mobile", "navitra", "digi", "object", "others"] {
            assert_eq!(v[format!("aprs-filter-{k}")], json!(true), "{k}");
        }
    }

    /// ★ The two text fields, which pad with **different bytes** — measured, not
    /// assumed. Decoding must stop at each field's own pad or the call sign
    /// picks up NULs and the power-on message picks up `0xFF`s.
    #[test]
    fn the_two_text_fields_decode_to_what_the_radio_shows() {
        let v = Value::Object(decoded());
        assert_eq!(v["aprs-my-callsign"], json!("WW8L-1"), "10 bytes, NUL-padded");
        assert_eq!(v["power-on-message"], json!("WW8L"), "8 bytes, 0xFF-padded");
    }

    /// And they re-encode with their own pad byte, filling the field.
    #[test]
    fn text_is_written_back_with_the_pad_byte_the_radio_uses() {
        let base = sample();
        let (out, changed) = patch(
            &base,
            &json!({ "aprs-my-callsign": "W1AW", "power-on-message": "HI" }),
        )
        .unwrap();
        assert_eq!(changed, 2);
        let aprs = &out.iter().find(|(w, _)| *w == W::Aprs).unwrap().1;
        let cfg = &out.iter().find(|(w, _)| *w == W::Config).unwrap().1;
        assert_eq!(&aprs[0x000..0x00A], b"W1AW\0\0\0\0\0\0");
        assert_eq!(&cfg[0x0E0..0x0E8], b"HI\xff\xff\xff\xff\xff\xff");
    }

    /// A call sign one character too long is REFUSED, not truncated. A silently
    /// shortened call sign goes on the air.
    #[test]
    fn text_too_long_for_the_field_is_refused() {
        let base = sample();
        let err = patch(&base, &json!({ "aprs-my-callsign": "WW8L-12345" })).unwrap_err();
        assert!(err.contains("10 characters") && err.contains("Menu 600"), "{err}");
        let err = patch(&base, &json!({ "power-on-message": "TOO LONG!" })).unwrap_err();
        assert!(err.contains("Menu 500"), "{err}");
        // Non-printable is refused too, rather than written as a control byte.
        assert!(patch(&base, &json!({ "power-on-message": "A\u{7}B" })).is_err());
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
            let mut wins = sample();
            wins.iter_mut().find(|(w, _)| *w == W::Aprs).unwrap().1[0x167] = mask;
            let mut m = Map::new();
            decode(&wins, &mut m);
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
    /// position records, the status texts and the config window's VFO settings
    /// are not this form's to rewrite.
    #[test]
    fn patching_leaves_every_byte_the_form_does_not_expose_alone() {
        let mut base = sample();
        {
            let aprs = &mut base.iter_mut().find(|(w, _)| *w == W::Aprs).unwrap().1;
            aprs[0x089] = b'H'; // status text 1
            aprs[0x474] = 0x08; // Sky Command tone
        }
        let cfg_before = base.iter().find(|(w, _)| *w == W::Config).unwrap().1.clone();

        let (out, changed) = patch(&base, &json!({ "aprs-temperature-unit": "Celsius" })).unwrap();
        assert_eq!(changed, 1);
        let aprs = &out.iter().find(|(w, _)| *w == W::Aprs).unwrap().1;
        assert_eq!(aprs[0x362], 0x01);
        assert_eq!(aprs[0x089], b'H');
        assert_eq!(aprs[0x474], 0x08);
        // ⚠ And the OTHER window was not touched at all.
        assert_eq!(
            out.iter().find(|(w, _)| *w == W::Config).unwrap().1,
            cfg_before,
            "an APRS-only change wrote into the config window"
        );
    }

    /// Six form fields share one byte, so clearing one must leave the other five.
    #[test]
    fn a_masked_field_changes_only_its_own_bit() {
        let base = sample();
        let (out, changed) = patch(&base, &json!({ "aprs-filter-digi": false })).unwrap();
        assert_eq!(changed, 1);
        assert_eq!(
            out.iter().find(|(w, _)| *w == W::Aprs).unwrap().1[0x167],
            0x3B,
            "only bit 2 should have cleared"
        );
    }

    /// A value round-trips through the form's labels and back to the same bytes.
    #[test]
    fn every_field_round_trips_through_its_labels() {
        let base = sample();
        let (out, changed) = patch(&base, &Value::Object(decoded())).unwrap();
        assert_eq!(changed, 0, "decoding and re-encoding must move nothing");
        assert_eq!(out, base);
    }

    /// An unlabelled stored value survives the round trip as a number. Refuse it
    /// and every later write fails, which is how the TH-D72 became unwritable.
    #[test]
    fn an_unlabelled_value_round_trips_as_a_number() {
        let mut base = sample();
        base.iter_mut().find(|(w, _)| *w == W::Aprs).unwrap().1[0x087] = 64;
        let mut m = Map::new();
        decode(&base, &mut m);
        assert_eq!(m["aprs-position-comment"], json!(64));
        let (out, _) = patch(&base, &Value::Object(m)).unwrap();
        assert_eq!(out.iter().find(|(w, _)| *w == W::Aprs).unwrap().1[0x087], 64);
    }

    #[test]
    fn a_value_outside_the_measured_range_is_refused() {
        let f = TMD710_IMAGE_FIELDS
            .iter()
            .find(|f| f.key == "aprs-ui-check-time")
            .unwrap();
        let err = encode_one(f, &json!(251)).unwrap_err();
        assert!(err.contains("0..=250") && err.contains("Menu 617"), "{err}");
        let band = TMD710_IMAGE_FIELDS
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
