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
//! sheet. **39 of the 66 individual settings in the 6xx/7xx range ship.** A row
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
//! | the 6xx/7xx image block holds | 34 | 66, of which **39 ship** |
//! | reached by neither | ~39 | the 1xx-5xx menus with no `MU` parameter |
//!
//! So the form is **95 controls**, which is not the same number as 39 settings:
//! menus 605 and 608 hold **five records each**, so three position settings
//! become fifteen controls and two status-text settings become ten. The census
//! counts settings; the form counts controls; `the_census_is_stated_rather_than_implied`
//! asserts the arithmetic between them so "95 fields" can never be reported as
//! coverage it is not.
//!
//! The 27 unshipped 6xx/7xx settings each have a row in the sheet's `## Owed`
//! table naming the check that settles it: three belong to menu 612's packet
//! path (see below), nine are located but seen at a single value, and the rest
//! are unlocated.
//!
//! ## WHERE THIS STOPS — read before adding anything (s134)
//!
//! **Tim called this done on 2026-09-06**, after the three fields below landed:
//! *"close enough mark it all as good, we'll deal with those unlikely bugs if
//! they occur."* That lifts the earlier "no PR until APRS is usable" gate and
//! settles the two checks listed under *Accepted unverified* — they are a
//! deliberate risk, not an oversight, and each still names the one command that
//! would close it.
//!
//! ### What s134 closed
//!
//! The gap that defined "usable" was: *you can set your call sign and not your
//! position, status text, symbol or path.* Three of those four are now measured
//! on the radio and shipped.
//!
//! | menu | setting | how it is anchored |
//! |---|---|---|
//! | 605 | MY POSITION ×5 | 8-byte `FF`-padded name, then `[deg][min][frac16 LE][hemisphere]` **twice**. The fraction is thousandths of a minute; latitude is `0`=N/`1`=S and longitude `0`=E/`1`=W, **all four read on the front panel**. Two slots poked with disjoint digits (12/34/321 + 98/12/654, then 56/7/890 + 123/45/670) |
//! | 608 | STATUS TEXT ×5 | array base `+0x08A`, record = `[42 text][1 unknown][1 TX rate]`. Padding measured off **three texts the radio itself wrote**; rate stores the DENOMINATOR (`00`=Off, `03` read `1/3`) |
//! | 610 | STATION ICON | the raw APRS symbol table + code, anchored twice (`/-`→House, `/>`→Car) and agreeing with the **published APRS spec** rather than any list in the manual |
//! | 624 | RX BEEP | all five indices, the manual's list **reversed** |
//!
//! ★★★ **`+0x165` is resolved: it is status text record 5's TX rate.** Three
//! hypotheses died on that byte — position comment (s129), TX rate (s131),
//! position limit (s132) — and every one failed for the same reason: **the record
//! boundary was off by one**, not the encoding. s131's "`05` reads `1/5`" was
//! right about the encoding and wrong about which record owned it. When a byte
//! resists three guesses, suspect the array around it, not the byte.
//!
//! ### ⚠ Menu 612 PACKET PATH is located and deliberately NOT shipped
//!
//! It is not one setting. The manual lists four types, each with its own
//! sub-fields, and the menu shows all four with a marker on the one in use:
//!
//! - `+0x421` type index — `00`=New-N and `01`=Relay measured. `02` and `03`
//!   **both fell back to New-N** while their string field was empty, which is the
//!   manual's documented behaviour and not a bad offset. Once a path string
//!   existed the byte held `03` through a front-panel `USE`, but the marker was
//!   never read afterwards, so index 3 has **indirect evidence only**.
//! - `+0x172` TOTAL HOPS — one value (`03`). ⚠⚠ poking `07` left menu 612 with
//!   **nothing selectable** until the block was restored, so 7 is out of range.
//! - `+0x184` the OTHERS path string — NUL-padded, and the radio **uppercases**
//!   it (`0vt` typed on the panel stored as `0VT`). Width unmeasured.
//! - `+0x174` WIDE 1-1 — went `01`→`02` when set ON, so it is **not** a 0/1
//!   boolean and the OFF value is unconfirmed.
//!
//! One short round settles all four: poke `+0x174` at `01`/`02`, `+0x172` at
//! `01`/`02`/`04`, set an ABBR for State/Section/Region, and read menu 612.
//!
//! ### ⚠ Accepted unverified — a decision, not an oversight
//!
//! **`d710_record_fields_write` has never run on a radio.** It exercises menu
//! 605's and 608's records through `write_settings` — the same call the profile
//! screen makes — into slot 3 of each, which is unused on this operator's radio,
//! and puts the as-found bytes back raw afterwards (the form cannot express
//! "FF-filled", because an empty field means *leave it alone*). It was written
//! after the cable came off the Mac, and Tim chose to ship without it.
//!
//! So what IS and IS NOT established for the 27 fields s134 added: every
//! **encoding** was measured on the radio, and the **codecs** are tested only
//! against a buffer built from the radio's own bytes. The offsets are guarded by
//! `the_record_strides_land_where_the_radio_puts_them` and by a whole-span
//! overlap check, which is why the residual risk was judged small — but a buffer
//! cannot prove the driver writes where it means to. If a position or status text
//! ever comes back wrong, run this FIRST; it is one command:
//!
//! ```text
//! D710_PORT=… cargo test --lib d710_record_fields_write -- --ignored --nocapture
//! ```
//!
//! Everything else in the settings path IS hardware-proven, including — as of
//! s134 — the `W::Config` window read, which had never been done by the driver:
//! it decoded `power-on-message` to `WW8L` and the call sign to `WW8L-1` off the
//! real radio, and the restore left both transports byte-identical.
//!
//! ### ⚠⚠ A hazard this module now defends against, and one it does not
//!
//! `patch` treats an **empty string as "not set"** and leaves the radio's bytes
//! alone. That is load-bearing: a profile the operator has never downloaded into
//! seeds every text field to `""` (`seedValues` → `fieldDefault`), and
//! `write_radio_settings` sends the profile as *saved* — so treating `""` as a
//! value would let a fresh profile blank the call sign, all five status texts and
//! all five position records in one write. Same shape as #90.
//!
//! ⚠ **The same seeding still pushes every `select` and `boolean` default**, and
//! that is NOT fixed here. A fresh D710 profile written to a radio would set ~60
//! settings to a schema default the operator never chose. It is a form-layer
//! problem, not this module's, and it is not specific to this radio.
//!
//! ### Not shipped, and not planned — each still names its check
//!
//! None of these blocks the model; they are here so a later session does not
//! rediscover them from scratch.
//!
//! - **`MU` p25 is menu 403 or 406** — change menu **403** on the front panel and
//!   read `MU`. ⚠ 403 is cross-band repeat; do not guess it.
//! - **A second tranche sits in `W::Config`**, which this driver already reads.
//!   CHIRP names contrast (504), PC port baud (519), visual scan (515), group
//!   link (203), S-meter squelch (105), WX alert (110) and repeater mode (403)
//!   inside the `0x0200` block. ⚠ CHIRP's *field* claims for this radio have
//!   never been checked, so each needs the factory-default cross-check first.
//! - Single-valued or unexplained: `+0x35D`, `+0x35E`, `+0x360`, `+0x361`,
//!   `+0x363`, `+0x00B`, `+0x35F`=`82`, and each record's own unknown byte —
//!   position idx8/idx19 and status text's 43rd.
//! - The ten group **names** and menu **203** itself are settings and unlocated.
//!
//! ### ★ How to work on this without wasting a radio session
//!
//! Batch **4-6 pokes across different menus in one pass**, then one walk of the
//! front panel. Always: distinct values, a control read, leave-and-re-enter
//! before believing a screen, and a step-aligned negative control at an edge.
//!
//! ★★ And **look for the A manual before asking the operator anything.** Menu
//! 612's four-field shape is in `TM-D710A_manual.txt` plus the G's PACKET PATH
//! section; a question was put to Tim that the manual on disk already answered.
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

    /// How many bytes this field occupies.
    ///
    /// Derived from the kind rather than stored, so a new multi-byte kind cannot
    /// be added while some caller goes on assuming one byte — which is what the
    /// overlap and volatile-state guards below both depend on.
    pub(crate) fn span(&self) -> usize {
        match self.kind {
            AK::Text { bytes, .. } => bytes,
            AK::LatLon { .. } => 5,
            AK::Symbol => 2,
            AK::Bool | AK::Bit { .. } | AK::Enum { .. } | AK::Uint { .. } | AK::Ctcss => 1,
        }
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
    /// Half of a menu 605 position: `[deg][min][frac lo][frac hi][hemisphere]`,
    /// five bytes, the fraction a 16-bit LITTLE-endian count of **thousandths of
    /// a minute**.
    ///
    /// Measured by poking a slot the operator was not using and reading the
    /// front panel: 12/34/321 came back as `12 34.32` and 98/12/654 as
    /// `098 12.65`, so the panel shows two decimals of a value stored with
    /// three. A second slot (56/7/890 and 123/45/670) confirmed it.
    ///
    /// ⚠ The hemisphere byte sits **after** its value, not before it, and the
    /// two hemispheres do not share a convention: latitude is `0`=N/`1`=S and
    /// longitude is `0`=E/`1`=W. Both were read on the screen for both fields.
    /// The record is symmetric — `[value][hemisphere]` twice — which is what
    /// made an earlier split that put both flags up front fit the operator's own
    /// data perfectly and predict the wrong thing.
    LatLon { lon: bool },
    /// A station icon: the raw APRS symbol **table** byte then **code** byte,
    /// exactly two printable characters.
    ///
    /// Not an index into the menu's icon list. `2F 2D` = `/-` reads House on the
    /// radio and a poked `2F 3E` = `/>` read Car — two anchors, and both agree
    /// with the published APRS symbol spec rather than with any list in the
    /// manual, so the encoding rests on a standard instead of on a printed
    /// order this radio's manual has already got wrong three different ways.
    Symbol,
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
        if f.span() > 1 {
            let Some(raw) = bytes_of(wins, f, f.span()) else { continue };
            let value = match f.kind {
                AK::Text { .. } | AK::Symbol => json!(trim_text(raw)),
                AK::LatLon { lon } => json!(decode_latlon(raw, lon)),
                _ => unreachable!("{} spans {} bytes but is not a multi-byte kind", f.key, f.span()),
            };
            out.insert(f.key.to_string(), value);
            continue;
        }
        let Some(&b) = bytes_of(wins, f, 1).map(|s| &s[0]) else { continue };
        let value = match &f.kind {
            // Handled above; the `continue` there is what makes these arms dead.
            AK::Text { .. } | AK::Symbol | AK::LatLon { .. } => {
                unreachable!("multi-byte kinds decode before this match")
            }
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

/// Text up to the first byte that is not printable ASCII.
///
/// ⚠ **Not up to `pad`.** `pad` is what the radio writes after text *it* wrote,
/// and that is not what fills a record the radio has never written: a status text
/// slot the operator has never used is `FF`-filled while a used one is
/// NUL-padded, and the power-on message pads with `FF` where the call sign pads
/// with `00`. Every one of those terminates here. [`encode_text`] refuses
/// non-printable input, so a non-printable byte inside one of these fields is
/// always padding or space the radio has never touched.
fn trim_text(raw: &[u8]) -> String {
    let end = raw.iter().position(|b| !(0x20..0x7F).contains(b)).unwrap_or(raw.len());
    raw[..end].iter().map(|b| *b as char).collect()
}

/// One position half as the form shows it — `"N 40 29.240"` — or `""` for a slot
/// the radio is not using.
///
/// An all-zero record is the radio's own empty slot; four of this operator's five
/// hold exactly that. It decodes to the empty string so the form shows a blank
/// rather than a spurious position on the equator, and [`encode_latlon`] writes
/// the zeros back for an empty string, so the round trip is exact.
fn decode_latlon(raw: &[u8], lon: bool) -> String {
    if raw.iter().all(|b| *b == 0) {
        return String::new();
    }
    let hemi = match (lon, raw[4]) {
        (false, 0) => 'N',
        (false, _) => 'S',
        (true, 0) => 'E',
        (true, _) => 'W',
    };
    // ⚠ Thousandths of a minute, LITTLE-endian, and the panel shows only two of
    // the three digits — poking 321 read back as `.32`. So the third digit is
    // real storage the radio will not display, and rounding it away here would
    // change a position the operator never edited.
    let frac = u16::from_le_bytes([raw[2], raw[3]]);
    // Zero-padded exactly as the radio's own screen shows it — three degree
    // digits for a longitude, two for a latitude, two minute digits for both.
    // Tim read `098 12.65` and `56 07.89` off the panel, and a form that renders
    // the same position differently from the radio is a form you cannot check
    // against the radio.
    let deg = if lon { format!("{:03}", raw[0]) } else { format!("{:02}", raw[0]) };
    format!("{hemi} {deg} {:02}.{frac:03}", raw[1])
}

/// `"N 40 29.240"` -> `[deg, min, frac lo, frac hi, hemisphere]`.
fn encode_latlon(f: &AF, v: &Value, lon: bool) -> Result<Vec<u8>, String> {
    let raw = v
        .as_str()
        .ok_or_else(|| format!("{} expects text, got {v}", f.display()))?;
    let s = raw.trim();
    if s.is_empty() {
        // The radio's own "unused slot". Reached only from a value that was
        // explicitly cleared, since `patch` skips an untouched empty field.
        return Ok(vec![0; 5]);
    }
    let shape = if lon { "W 104 55.840" } else { "N 40 29.240" };
    let bad = || format!("{} should look like \"{shape}\"; got {s:?}", f.display());

    // The hemisphere letter is taken from either end: "40 29.240 N" is how a lot
    // of people write it, and refusing that teaches an operator nothing.
    let mut body = s.to_ascii_uppercase();
    let letters = if lon { ['E', 'W'] } else { ['N', 'S'] };
    let hemi = if body.starts_with(letters) {
        body.remove(0)
    } else if body.ends_with(letters) {
        body.pop().expect("non-empty")
    } else {
        return Err(format!(
            "{} needs {} or {} for the hemisphere; got {s:?}",
            f.display(),
            letters[0],
            letters[1]
        ));
    };

    let mut parts = body.split_whitespace();
    let (Some(d), Some(m), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(bad());
    };
    let (whole, frac) = match m.split_once('.') {
        Some((whole, fr)) => {
            if fr.is_empty() || fr.len() > 3 || !fr.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad());
            }
            // Left-aligned, because it is a decimal fraction: ".5" is 500
            // thousandths of a minute, not 5.
            (whole, format!("{fr:0<3}").parse::<u16>().map_err(|_| bad())?)
        }
        None => (m, 0),
    };
    let deg: u16 = d.parse().map_err(|_| bad())?;
    let min: u16 = whole.parse().map_err(|_| bad())?;

    let deg_max = if lon { 180 } else { 90 };
    if deg > deg_max || (deg == deg_max && (min > 0 || frac > 0)) {
        return Err(format!("{} is past {deg_max}\u{b0}; got {s:?}", f.display()));
    }
    if min > 59 {
        return Err(format!("{} has {min} minutes; the radio stores 0-59", f.display()));
    }
    let [lo, hi] = frac.to_le_bytes();
    Ok(vec![deg as u8, min as u8, lo, hi, u8::from(hemi == 'S' || hemi == 'W')])
}

/// `"/-"` -> the two raw APRS symbol bytes, table then code.
fn encode_symbol(f: &AF, v: &Value) -> Result<Vec<u8>, String> {
    let s = v
        .as_str()
        .ok_or_else(|| format!("{} expects text, got {v}", f.display()))?;
    let c: Vec<char> = s.chars().collect();
    // Exactly two, not "at most two": a symbol is a table byte AND a code byte,
    // and half of one is not a lesser symbol, it is a different one.
    if c.len() != 2 || c.iter().any(|c| !(' '..='~').contains(c)) {
        return Err(format!(
            "{} is an APRS symbol table and code \u{2014} exactly two characters, \
             like \"/-\" for a house or \"/>\" for a car; got {s:?}",
            f.display()
        ));
    }
    Ok(vec![c[0] as u8, c[1] as u8])
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

/// One multi-byte field's value as the bytes the radio stores, or `None` when
/// the field is a single byte and belongs to [`encode_one`].
fn encode_multi(f: &AF, v: &Value) -> Option<Result<Vec<u8>, String>> {
    match f.kind {
        AK::Text { .. } => Some(encode_text(f, v)),
        AK::Symbol => Some(encode_symbol(f, v)),
        AK::LatLon { lon } => Some(encode_latlon(f, v, lon)),
        _ => None,
    }
}

/// One form value as the byte the radio stores.
fn encode_one(f: &AF, v: &Value) -> Result<u8, String> {
    Ok(match &f.kind {
        AK::Text { .. } | AK::Symbol | AK::LatLon { .. } => {
            unreachable!("multi-byte kinds go through encode_multi")
        }
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
        // ⚠⚠ An empty string means **"not set"**, and the radio's own bytes are
        // left exactly as they came. It does NOT mean "erase this field".
        //
        // This is not tidiness. A profile the operator has never downloaded into
        // seeds every text field to `""` (`seedValues` -> `fieldDefault`), and
        // `write_radio_settings` sends the profile as SAVED — so treating `""` as
        // a value would let a fresh profile blank the operator's call sign, all
        // five status texts and all five position records in one write, none of
        // which this form had ever shown them. Same shape as #90.
        //
        // The cost is that the form cannot clear one of these fields, which is
        // the far cheaper half of the trade: nothing here has a useful empty
        // value on the air, and an unused position slot is already unused.
        if v.as_str() == Some("") {
            continue;
        }
        let Some((_, buf)) = out.iter_mut().find(|(w, _)| *w == f.win) else { continue };

        if let Some(encoded) = encode_multi(f, v) {
            let encoded = encoded?;
            let n = f.span();
            debug_assert_eq!(encoded.len(), n, "{} encoded {} bytes", f.key, encoded.len());
            if buf[f.off..f.off + n] != encoded[..] {
                buf[f.off..f.off + n].copy_from_slice(&encoded);
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
        // The operator's own two position records and three of his status texts,
        // byte for byte out of `progfull-54397.bin`. Synthetic values would test
        // the parser against itself; these are what the radio holds and what its
        // screen was showing while they were read.
        aprs[0x01C..0x030].copy_from_slice(
            b"THESHACK\x00\x28\x1d\xf0\x00\x00\x68\x37\x48\x03\x01\x00",
        );
        aprs[0x030..0x044].copy_from_slice(
            b"RancH\xff\xff\xff\x00\x28\x1a\x08\x02\x00\x68\x3b\x96\x00\x01\x00",
        );
        let one = b"IN THE SHACK ON 447.275, 3171 DMR, ";
        aprs[0x08A..0x08A + one.len()].copy_from_slice(one);
        aprs[0x0B5] = 0x01; // status text 1 TX rate = 1/1
        // ⚠ Record 3 as the radio leaves a slot it has NEVER written: FF-filled,
        // not NUL-padded. That is the case the decoder's terminator has to cover
        // and the reason it stops at the first non-printable byte instead of at
        // this field's `pad`.
        aprs[0x0E2..0x0E2 + 42].fill(0xFF);
        aprs[0x169..0x16B].copy_from_slice(b"/-"); // station icon: a house
        aprs[0x350] = 0x03; // 624 RX beep = Message only
        vec![(W::Config, cfg), (W::Aprs, aprs)]
    }

    /// One field by key, so a codec test names the field the form names.
    fn field(key: &str) -> &'static AF {
        TMD710_IMAGE_FIELDS
            .iter()
            .find(|f| f.key == key)
            .unwrap_or_else(|| panic!("no field {key:?}"))
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
                // Both render as text, and both are unguessable without the
                // example the schema carries as a placeholder — so the
                // placeholder is part of what has to agree, not decoration.
                AK::Symbol | AK::LatLon { .. } => {
                    assert_eq!(e["type"], "text", "{}", f.key);
                    assert!(
                        e["placeholder"].as_str().is_some_and(|s| !s.is_empty()),
                        "{} has no placeholder, so its format is unguessable",
                        f.key
                    );
                    // ⚠ A symbol is exactly two characters and `encode_symbol`
                    // refuses anything else, so the form must refuse it too — it
                    // shipped as 12, which meant the operator learned that at write
                    // time. A coordinate IS parsed, so its width is the canonical
                    // form's ("W 180 59.999").
                    let want = if matches!(f.kind, AK::Symbol) { 2 } else { 12 };
                    assert_eq!(e["max_length"], json!(want), "{}", f.key);
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
            60,
            "controls, not settings — the five-record menus contribute five rows each. \
             If this moved, update the census in the module doc and the ## Owed rows \
             in APRS-MEASURED.md."
        );

        // ⚠ The 66 denominator counts each menu's DISTINCT settings, which is how
        // CENSUS.md itemised it: menu 605 contributes NAME / LATITUDE / LONGITUDE
        // and menu 608 contributes TEXT / TX RATE — once each, not once per
        // record. So the per-record menus have to collapse before the two numbers
        // can honestly be compared, and stating that here is what stops "60
        // fields" from being quietly reported as coverage it is not.
        let per_record =
            TMD710_IMAGE_FIELDS.iter().filter(|f| matches!(f.menu, "605" | "608")).count();
        assert_eq!(per_record, 25, "five records of 3 position and 2 status-text settings");
        let distinct = TMD710_IMAGE_FIELDS.len() - per_record + 5;
        assert_eq!(
            distinct - 1,
            39,
            "39 of the 66 individual settings in the radio's 6xx/7xx menus. The -1 is \
             menu 500's power-on message, which comes from the config window and is \
             not an APRS setting at all."
        );

        let schema: Vec<Value> =
            serde_json::from_str(crate::seed::TMD710_SETTINGS_SCHEMA).expect("schema parses");
        let fields = schema.iter().filter(|e| e["type"] != "section").count();
        assert_eq!(fields, 60 + 35, "the form is both transports");
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
            for off in f.off..f.off + f.span() {
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

        // ⚠ Every byte of every field, not just the first. Twenty of these
        // fields are multi-byte records laid out at a fixed stride, so the
        // failure this has to catch is a record whose offset arithmetic is off
        // and which therefore runs into its neighbour — invisible to a check
        // that only compares starting offsets, and it would corrupt the field
        // next door on the first write.
        let mut owner: std::collections::HashMap<(u16, usize, i16), &str> =
            std::collections::HashMap::new();
        for f in TMD710_IMAGE_FIELDS {
            let bit = match f.kind {
                AK::Bit { bit } => i16::from(bit),
                _ => -1,
            };
            for off in f.off..f.off + f.span() {
                if let Some(prev) = owner.insert((f.win.base(), off, bit), f.key) {
                    panic!("{} and {} both claim {:?} +0x{off:03X}", f.key, prev, f.win);
                }
            }
        }
        assert!(
            TMD710_IMAGE_FIELDS.iter().all(|f| f.off + f.span() <= f.win.len()),
            "a field runs past the end of its window"
        );
        let _ = n;
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

    /// The operator's own records, decoded to what his radio's screen shows and
    /// re-encoded to the very same bytes.
    #[test]
    fn a_position_record_round_trips_through_the_operators_own_bytes() {
        let v = Value::Object(decoded());
        assert_eq!(v["aprs-position-1-name"], json!("THESHACK"));
        assert_eq!(v["aprs-position-1-lat"], json!("N 40 29.240"));
        assert_eq!(v["aprs-position-1-lon"], json!("W 104 55.840"));
        // FF-padded name, and a fraction whose low byte is zero — the case a
        // big-endian reading would decode as 8.192 minutes instead of 0.520.
        assert_eq!(v["aprs-position-2-name"], json!("RancH"));
        assert_eq!(v["aprs-position-2-lat"], json!("N 40 26.520"));
        assert_eq!(v["aprs-position-2-lon"], json!("W 104 59.150"));

        let base = sample();
        let (out, changed) = patch(&base, &v).expect("re-encode");
        assert_eq!(changed, 0, "decoding and re-encoding the radio's own bytes moved one");
        assert_eq!(out, base);
    }

    /// ★ The measurement that named this layout, kept as a test: the exact bytes
    /// poked into a slot the operator was not using, and the exact strings his
    /// front panel then showed.
    ///
    /// ⚠ The panel showed `12 34.32` for a stored 321 — two digits of a value
    /// held in three. The third digit is real storage, so it is preserved here
    /// rather than rounded to what the screen can draw.
    #[test]
    fn the_poked_positions_decode_to_what_the_front_panel_showed() {
        // lat 12/34/321 with hemisphere 1, lon 98/12/654 with hemisphere 1
        assert_eq!(decode_latlon(&[12, 34, 0x41, 0x01, 1], false), "S 12 34.321");
        assert_eq!(decode_latlon(&[98, 12, 0x8E, 0x02, 1], true), "W 098 12.654");
        // and the same slots with hemisphere 0, which read N and E on the screen
        assert_eq!(decode_latlon(&[12, 34, 0x41, 0x01, 0], false), "N 12 34.321");
        assert_eq!(decode_latlon(&[98, 12, 0x8E, 0x02, 0], true), "E 098 12.654");
        // the second slot, a different set of digits entirely
        assert_eq!(decode_latlon(&[56, 7, 0x7A, 0x03, 1], false), "S 56 07.890");
        assert_eq!(decode_latlon(&[123, 45, 0x9E, 0x02, 0], true), "E 123 45.670");
    }

    #[test]
    fn a_position_is_accepted_the_way_people_actually_write_one() {
        let lat = field("aprs-position-1-lat");
        let want = vec![40, 29, 0xF0, 0x00, 0];
        for s in ["N 40 29.240", "n 40 29.240", "40 29.240 N", "  N   40   29.240  "] {
            assert_eq!(encode_latlon(lat, &json!(s), false).expect(s), want, "{s:?}");
        }
        // A fraction is DECIMAL, so ".5" is 500 thousandths of a minute, not 5.
        assert_eq!(
            encode_latlon(lat, &json!("N 40 29.5"), false).expect("short fraction"),
            vec![40, 29, 0xF4, 0x01, 0]
        );
        assert_eq!(
            encode_latlon(lat, &json!("N 40 29"), false).expect("no fraction"),
            vec![40, 29, 0, 0, 0]
        );
    }

    #[test]
    fn a_position_the_radio_cannot_store_is_refused_rather_than_clamped() {
        let lat = field("aprs-position-1-lat");
        let lon = field("aprs-position-1-lon");
        for s in ["E 40 29.240", "40 29.240", "N 91 00.000", "N 40 60.000", "N 40 29.2405"] {
            assert!(encode_latlon(lat, &json!(s), false).is_err(), "{s:?} was accepted");
        }
        // A longitude reaches 180 and takes E/W, not N/S — the two halves do not
        // share a range or a letter pair.
        assert!(encode_latlon(lon, &json!("N 104 55.840"), true).is_err());
        assert!(encode_latlon(lon, &json!("W 104 55.840"), true).is_ok());
        assert!(encode_latlon(lon, &json!("W 181 00.000"), true).is_err());
        assert!(encode_latlon(lat, &json!("N 104 55.840"), false).is_err(), "past 90");
    }

    /// An unused slot is all zeros on the radio and blank in the form, both ways.
    #[test]
    fn an_unused_position_slot_is_blank_in_the_form_and_zeros_on_the_radio() {
        let v = Value::Object(decoded());
        assert_eq!(v["aprs-position-3-lat"], json!(""), "slot 3 is unused");
        assert_eq!(v["aprs-position-5-lon"], json!(""));
        assert_eq!(
            encode_latlon(field("aprs-position-3-lat"), &json!(""), false).expect("blank"),
            vec![0; 5]
        );
    }

    /// A symbol is a table byte AND a code byte; half of one is a different
    /// symbol, not a shorter one.
    #[test]
    fn a_station_icon_is_exactly_two_characters() {
        assert_eq!(Value::Object(decoded())["aprs-station-icon"], json!("/-"));
        let f = field("aprs-station-icon");
        assert_eq!(encode_symbol(f, &json!("/>")).expect("car"), b"/>".to_vec());
        assert_eq!(encode_symbol(f, &json!("\\K")).expect("factory"), b"\\K".to_vec());
        for s in ["/", "", "/->", "/\n"] {
            assert!(encode_symbol(f, &json!(s)).is_err(), "{s:?} was accepted");
        }
    }

    /// ⚠ A slot the radio has never written is FF-filled, while one it wrote is
    /// NUL-padded — so a decoder that trimmed this field's `pad` would hand the
    /// form 42 characters of `ÿ` for an empty status text.
    #[test]
    fn a_status_text_the_radio_never_wrote_reads_blank_rather_than_ff() {
        let v = Value::Object(decoded());
        assert_eq!(v["aprs-status-text-1"], json!("IN THE SHACK ON 447.275, 3171 DMR, "));
        assert_eq!(v["aprs-status-text-3"], json!(""), "an FF-filled record");
        // The rate stores the DENOMINATOR, which is what being off by one record
        // hid for two sessions.
        assert_eq!(v["aprs-status-text-1-rate"], json!("1/1"));
        assert_eq!(v["aprs-status-text-3-rate"], json!("Off"));
        // ★ And record 5's rate is `+0x165`, the byte this project misnamed three
        // times. It falls out of the array formula rather than being asserted.
        assert_eq!(field("aprs-status-text-5-rate").off, 0x165);
        assert_eq!(field("aprs-status-text-5").off, 0x13A);
    }

    /// ⚠⚠ The rule that stops a fresh profile from wiping the radio.
    ///
    /// A profile the operator has never downloaded into seeds every text field to
    /// `""`, and `write_radio_settings` sends the profile as saved. If `""` were a
    /// value, that write would blank the call sign, all five status texts and all
    /// five position records in one go — none of which the form had ever shown
    /// them. Same shape as #90.
    #[test]
    fn an_empty_field_leaves_the_radios_own_bytes_alone() {
        let base = sample();
        let blank = json!({
            "aprs-my-callsign": "",
            "power-on-message": "",
            "aprs-status-text-1": "",
            "aprs-position-1-name": "",
            "aprs-position-1-lat": "",
            "aprs-position-1-lon": "",
            "aprs-station-icon": "",
        });
        let (out, changed) = patch(&base, &blank).expect("a blank profile must not fail");
        assert_eq!(changed, 0, "a blank field counted as a change");
        assert_eq!(out, base, "a blank profile rewrote the radio's own bytes");

        // And a value that IS set still lands, so this is not a blanket skip.
        let (out, changed) =
            patch(&base, &json!({ "aprs-status-text-1": "CQ" })).expect("a real value");
        assert_eq!(changed, 1);
        let (_, aprs) = out.iter().find(|(w, _)| *w == W::Aprs).expect("aprs window");
        assert_eq!(&aprs[0x08A..0x08C], b"CQ");
        assert!(aprs[0x08C..0x0B4].iter().all(|b| *b == 0), "the rest pads as the radio pads");
        assert_eq!(aprs[0x0B5], 0x01, "the neighbouring TX rate byte was not touched");
    }

    /// A field that ran into its neighbour would corrupt it on the first write,
    /// and twenty of these are records at a fixed stride.
    #[test]
    fn the_record_strides_land_where_the_radio_puts_them() {
        for n in 0..5u8 {
            let i = n + 1;
            let b = 0x01C + usize::from(n) * 20;
            assert_eq!(field(&format!("aprs-position-{i}-name")).off, b);
            assert_eq!(field(&format!("aprs-position-{i}-lat")).off, b + 9);
            assert_eq!(field(&format!("aprs-position-{i}-lon")).off, b + 14);
            let s = 0x08A + usize::from(n) * 44;
            assert_eq!(field(&format!("aprs-status-text-{i}")).off, s);
            assert_eq!(field(&format!("aprs-status-text-{i}-rate")).off, s + 43);
        }
    }

}
