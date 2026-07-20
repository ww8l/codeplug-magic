//! UV-5R non-channel settings encoder (Phase C).
//!
//! Ports CHIRP's `uv5r.py` `set_settings` so the radio profile's 74 editable
//! settings (the `non_channel_settings` JSON, keyed by CHIRP `RadioSetting`
//! names) can be written straight to the radio. The values are encoded into the
//! freshly-downloaded image — the same round-tripped buffer the channel encoder
//! patches — so only the bytes we explicitly set change; everything else is
//! exactly what we read back from the radio.
//!
//! Memory map. CHIRP's `MEM_FORMAT` `#seekto` values are IMAGE offsets: the
//! image keeps the 8-byte ident prefix, so radio address 0x0000 is at image
//! 0x0008 (the channel struct is `#seekto 0x0008`). We therefore use the seektos
//! verbatim — do NOT re-add the +8. The aux block, read from radio 0x1EC0, is
//! appended after names at image 0x1808, so an aux address maps to
//! `0x1808 + (addr - 0x1EC0)`:
//!   - `settings`  struct @ image 0x0E28
//!   - `wmchannel` struct @ image 0x0E7E
//!   - `vfoa`/`vfob` @ image 0x0F10/0x0F30
//!   - `ani` (DTMF)  @ image 0x0C88
//!   - `pttid[15]`   @ image 0x0B08
//!   - `poweron_msg` @ image 0x1828 (radio 0x1EE0)
//!   - `limits_new`  @ image 0x1908 (radio 0x1FC0)

use std::collections::HashMap;

use serde_json::Value;

use crate::radios::driver::{SettingsCapture, SettingsReader};

use super::{download, ident_radio, open_port, BaofengUv5r};

impl SettingsReader for BaofengUv5r {
    /// Download the full image and decode the profile's settings out of it. One
    /// session: the image doubles as the backup the command layer saves, so the
    /// port is never opened twice for a single read.
    fn read_settings(&self, port: &str, schema_json: &str) -> Result<SettingsCapture, String> {
        let mut p = open_port(port)?;
        let (magic, ident) = ident_radio(&mut *p)?;
        if !magic.starts_with("UV5R") {
            return Err("the connected radio did not identify as a UV-5R".into());
        }
        let image = download(&mut *p, &ident)?;
        let settings = decode_settings_from_image(&image, schema_json)?;
        Ok(SettingsCapture {
            settings,
            backup: image,
            backup_ext: "img",
        })
    }
}

// Aux-block relocation: the bytes read from radio 0x1EC0 onward are stored in
// the image immediately after the main block + names (which end at 0x1808).
pub const AUX_IMAGE_BASE: usize = 0x1808;
pub const AUX_RADIO_BASE: u16 = 0x1EC0;

/// Radio-address ranges to upload when settings are written. These mirror
/// CHIRP's `_do_upload` main ranges (converted from image offsets to radio
/// addresses) and cover channels, names, DTMF, and the whole settings block.
/// The two skipped gaps (radio 0x0CF0–0x0D00 and 0x0DF0–0x0E00) are calibration
/// bytes CHIRP never writes.
pub const SETTINGS_MAIN_RANGES: &[(u16, u16)] =
    &[(0x0000, 0x0CF0), (0x0D00, 0x0DF0), (0x0E00, 0x1800)];

/// Aux-block ranges holding the power-on message (0x1EE0) and the "new" band
/// limits (0x1FC0). Written in addition to the main ranges.
pub const SETTINGS_AUX_RANGES: &[(u16, u16)] = &[(0x1EE0, 0x1EF0), (0x1FC0, 0x1FD0)];

/// DTMF code character set used by CHIRP to map code digits to byte values.
const DTMF_CHARS: &[u8] = b"0123456789 *#ABCD";

/// How one setting key is stored in the image.
enum Loc {
    /// Whole byte at `off`.
    Byte(usize),
    /// `width` bits at `shift` within the byte at `off` (others preserved).
    Bits(usize, u8, u8),
    /// VFO frequency: 8 single-digit bytes at `off` (big-endian), plus a band
    /// bit at `band` set from the frequency. Value is a MHz string.
    Freq { off: usize, band: (usize, u8) },
    /// VFO offset: 6 single-digit bytes at `off` (big-endian). MHz string.
    Offset(usize),
    /// Two big-endian BCD bytes at `off` (band limit, MHz). Integer value.
    Bbcd(usize),
    /// DTMF code: `len` bytes at `off`, each a `DTMF_CHARS` index; pad 0xFF.
    Dtmf(usize, usize),
    /// ASCII text (power-on message): `len` bytes at `off`; pad 0xFF.
    Text(usize, usize),
}

// CHIRP `#seekto` values are IMAGE offsets (the image keeps the 8-byte ident
// prefix, so radio address 0x0000 sits at image 0x0008 — exactly like our
// channel struct at #seekto 0x0008). These are the struct seektos verbatim; do
// NOT add the +8 ident offset, it is already baked into the seekto.
const SETTINGS: usize = 0x0E28; // `settings` struct
const WMCH: usize = 0x0E7E; // `wmchannel`
const VFOA: usize = 0x0F10; // `vfoa`
const VFOB: usize = 0x0F30; // `vfob`
const ANI: usize = 0x0C88; // `ani`
const PTTID: usize = 0x0B08; // `pttid[0]`
const PONMSG: usize = 0x1828; // `poweron_msg`
const LIMITS: usize = 0x1908; // `limits_new`

/// Resolve a setting key to its storage location. Returns `None` for keys we
/// don't write (unknown / read-only), which are skipped.
fn loc(key: &str) -> Option<Loc> {
    // pttid/<n>.code — 15 DTMF codes, 16 bytes apart, 5-byte codes.
    if let Some(rest) = key.strip_prefix("pttid/") {
        if let Some(idx) = rest.strip_suffix(".code").and_then(|n| n.parse::<usize>().ok()) {
            if idx < 15 {
                return Some(Loc::Dtmf(PTTID + idx * 16, 5));
            }
        }
        return None;
    }

    Some(match key {
        // ---- settings struct (whole bytes) ----
        "squelch" => Loc::Byte(SETTINGS),
        "save" => Loc::Byte(SETTINGS + 0x03),
        "vox" => Loc::Byte(SETTINGS + 0x04),
        "abr" => Loc::Byte(SETTINGS + 0x06),
        "tdr" => Loc::Byte(SETTINGS + 0x07),
        "beep" => Loc::Byte(SETTINGS + 0x08),
        "timeout" => Loc::Byte(SETTINGS + 0x09),
        "voice" => Loc::Byte(SETTINGS + 0x0E),
        "dtmfst" => Loc::Byte(SETTINGS + 0x10),
        "screv" => Loc::Bits(SETTINGS + 0x12, 0, 2),
        "pttlt" => Loc::Byte(SETTINGS + 0x14),
        "mdfa" => Loc::Byte(SETTINGS + 0x15),
        "mdfb" => Loc::Byte(SETTINGS + 0x16),
        "bcl" => Loc::Byte(SETTINGS + 0x17),
        "autolk" => Loc::Byte(SETTINGS + 0x18),
        "wtled" => Loc::Byte(SETTINGS + 0x1D),
        "rxled" => Loc::Byte(SETTINGS + 0x1E),
        "txled" => Loc::Byte(SETTINGS + 0x1F),
        "almod" => Loc::Byte(SETTINGS + 0x20),
        "tdrab" => Loc::Byte(SETTINGS + 0x22),
        "ste" => Loc::Byte(SETTINGS + 0x23),
        "rpste" => Loc::Byte(SETTINGS + 0x24),
        "rptrl" => Loc::Byte(SETTINGS + 0x25),
        "roger" => Loc::Byte(SETTINGS + 0x27),
        "workmode" => Loc::Byte(SETTINGS + 0x2C),
        "keylock" => Loc::Byte(SETTINGS + 0x2D),
        // ---- packed flag byte at settings+0x2A (image 0x0E5A) ----
        "displayab" => Loc::Bits(SETTINGS + 0x2A, 7, 1),
        "fmradio" => Loc::Bits(SETTINGS + 0x2A, 4, 1),
        "reset" => Loc::Bits(SETTINGS + 0x2A, 1, 1),
        "menu" => Loc::Bits(SETTINGS + 0x2A, 0, 1),
        // ---- work-mode channels ----
        "wmchannel.mrcha" => Loc::Bits(WMCH, 0, 7),
        "wmchannel.mrchb" => Loc::Bits(WMCH + 1, 0, 7),
        // ---- VFO A / B ----
        "vfoa.freq" => Loc::Freq { off: VFOA, band: (VFOA + 0x12, 0) },
        "vfob.freq" => Loc::Freq { off: VFOB, band: (VFOB + 0x12, 0) },
        "vfoa.offset" => Loc::Offset(VFOA + 0x08),
        "vfob.offset" => Loc::Offset(VFOB + 0x08),
        "vfoa.sftd" => Loc::Bits(VFOA + 0x14, 4, 2),
        "vfob.sftd" => Loc::Bits(VFOB + 0x14, 4, 2),
        "vfoa.scode" => Loc::Bits(VFOA + 0x14, 0, 4),
        "vfob.scode" => Loc::Bits(VFOB + 0x14, 0, 4),
        "vfoa.step" => Loc::Bits(VFOA + 0x16, 4, 3),
        "vfob.step" => Loc::Bits(VFOB + 0x16, 4, 3),
        "vfoa.txpower" => Loc::Bits(VFOA + 0x17, 7, 1),
        "vfob.txpower" => Loc::Bits(VFOB + 0x17, 7, 1),
        "vfoa.widenarr" => Loc::Bits(VFOA + 0x17, 6, 1),
        "vfob.widenarr" => Loc::Bits(VFOB + 0x17, 6, 1),
        // ---- DTMF / ANI ----
        "ani.code" => Loc::Dtmf(ANI + 0x2A, 5),
        "ani.alarmcode" => Loc::Dtmf(ANI + 0x0A, 3),
        "ani.aniid" => Loc::Bits(ANI + 0x2F, 0, 2),
        "ani.dtmfon" => Loc::Byte(ANI + 0x32),
        "ani.dtmfoff" => Loc::Byte(ANI + 0x33),
        // ---- power-on message ----
        "poweron_msg.line1" => Loc::Text(PONMSG, 7),
        "poweron_msg.line2" => Loc::Text(PONMSG + 7, 7),
        // ---- band limits (new) ----
        "limits.vhf.enable" => Loc::Byte(LIMITS),
        "limits.vhf.lower" => Loc::Bbcd(LIMITS + 1),
        "limits.vhf.upper" => Loc::Bbcd(LIMITS + 3),
        "limits.uhf.enable" => Loc::Byte(LIMITS + 5),
        "limits.uhf.lower" => Loc::Bbcd(LIMITS + 6),
        "limits.uhf.upper" => Loc::Bbcd(LIMITS + 8),
        _ => return None,
    })
}

/// A select field's option labels, indexed by the schema, so a stored label
/// (e.g. "Purple") can be turned back into the byte value the radio expects.
struct SelectOptions(HashMap<String, Vec<String>>);

impl SelectOptions {
    fn from_schema(schema_json: &str) -> Result<Self, String> {
        let schema: Value = serde_json::from_str(schema_json)
            .map_err(|e| format!("invalid settings schema JSON: {e}"))?;
        let arr = schema
            .as_array()
            .ok_or("settings schema is not a JSON array")?;
        let mut map = HashMap::new();
        for field in arr {
            let key = field.get("key").and_then(Value::as_str);
            let opts = field.get("options").and_then(Value::as_array);
            if let (Some(key), Some(opts)) = (key, opts) {
                let labels = opts
                    .iter()
                    .filter_map(|o| o.as_str().map(str::to_string))
                    .collect();
                map.insert(key.to_string(), labels);
            }
        }
        Ok(Self(map))
    }

    fn index_of(&self, key: &str, label: &str) -> Option<i64> {
        self.0
            .get(key)
            .and_then(|opts| opts.iter().position(|o| o == label))
            .map(|i| i as i64)
    }
}

/// Encode the profile's `non_channel_settings` into `image`. Returns the number
/// of fields applied (skipping unknown keys and empty free-text fields, which
/// leave the round-tripped bytes untouched). Never panics: out-of-range offsets
/// and unresolvable values are skipped so a partial/odd profile can't corrupt
/// the write.
pub fn apply_profile_settings(
    image: &mut [u8],
    schema_json: &str,
    settings_json: &str,
) -> Result<usize, String> {
    let options = SelectOptions::from_schema(schema_json)?;
    let settings: Value = serde_json::from_str(settings_json)
        .map_err(|e| format!("invalid profile settings JSON: {e}"))?;
    let obj = settings
        .as_object()
        .ok_or("profile settings is not a JSON object")?;

    let mut applied = 0usize;
    for (key, value) in obj {
        let Some(loc) = loc(key) else { continue };
        if apply_one(image, key, &loc, value, &options) {
            applied += 1;
        }
    }
    Ok(applied)
}

/// Apply one setting; returns true if a byte was written.
fn apply_one(
    image: &mut [u8],
    key: &str,
    loc: &Loc,
    value: &Value,
    options: &SelectOptions,
) -> bool {
    match loc {
        Loc::Byte(off) => match resolve_int(key, value, options) {
            Some(n) => put(image, *off, n as u8),
            None => false,
        },
        Loc::Bits(off, shift, width) => match resolve_int(key, value, options) {
            Some(n) => put_bits(image, *off, *shift, *width, n as u8),
            None => false,
        },
        Loc::Bbcd(off) => match resolve_int(key, value, options) {
            Some(n) => put_bbcd2(image, *off, n.max(0) as u32),
            None => false,
        },
        Loc::Freq { off, band } => match value.as_str().map(str::trim) {
            Some(s) if !s.is_empty() => put_freq(image, *off, *band, s),
            _ => false, // blank VFO frequency: leave as read
        },
        Loc::Offset(off) => match value.as_str().map(str::trim) {
            Some(s) if !s.is_empty() => put_offset(image, *off, s),
            _ => false,
        },
        Loc::Dtmf(off, len) => match value.as_str() {
            Some(s) => put_dtmf(image, *off, *len, s),
            None => false,
        },
        Loc::Text(off, len) => match value.as_str() {
            Some(s) => put_text(image, *off, *len, s),
            None => false,
        },
    }
}

/// Resolve a JSON value to an integer byte value: booleans → 0/1, numbers as-is,
/// strings via the select-option index.
fn resolve_int(key: &str, value: &Value, options: &SelectOptions) -> Option<i64> {
    match value {
        Value::Bool(b) => Some(*b as i64),
        Value::Number(n) => n.as_i64(),
        Value::String(s) => options.index_of(key, s),
        _ => None,
    }
}

fn put(image: &mut [u8], off: usize, v: u8) -> bool {
    if off < image.len() {
        image[off] = v;
        true
    } else {
        false
    }
}

fn put_bits(image: &mut [u8], off: usize, shift: u8, width: u8, v: u8) -> bool {
    if off >= image.len() {
        return false;
    }
    let mask = ((1u16 << width) - 1) as u8;
    let cur = image[off];
    image[off] = (cur & !(mask << shift)) | ((v & mask) << shift);
    true
}

/// Two big-endian BCD bytes (4 decimal digits) for a band limit in MHz.
fn put_bbcd2(image: &mut [u8], off: usize, mhz: u32) -> bool {
    if off + 1 >= image.len() {
        return false;
    }
    let d3 = (mhz / 1000 % 10) as u8;
    let d2 = (mhz / 100 % 10) as u8;
    let d1 = (mhz / 10 % 10) as u8;
    let d0 = (mhz % 10) as u8;
    image[off] = (d3 << 4) | d2;
    image[off + 1] = (d1 << 4) | d0;
    true
}

/// VFO frequency: 8 single-decimal-digit bytes (big-endian), value = Hz/10.
fn put_freq(image: &mut [u8], off: usize, band: (usize, u8), mhz_str: &str) -> bool {
    let Ok(mhz) = mhz_str.parse::<f64>() else {
        return false;
    };
    let hz = (mhz * 1_000_000.0).round() as u64;
    let mut value = hz / 10;
    if off + 8 > image.len() {
        return false;
    }
    for i in (0..8).rev() {
        image[off + i] = (value % 10) as u8;
        value /= 10;
    }
    // band bit: set for >= 400 MHz (UHF), like CHIRP's apply_freq.
    put_bits(image, band.0, band.1, 1, (hz >= 400_000_000) as u8);
    true
}

/// VFO offset: 6 single-decimal-digit bytes (big-endian), value = Hz/1000.
fn put_offset(image: &mut [u8], off: usize, mhz_str: &str) -> bool {
    let Ok(mhz) = mhz_str.parse::<f64>() else {
        return false;
    };
    let mut value = (mhz * 1_000.0).round() as u64; // MHz -> kHz units
    if off + 6 > image.len() {
        return false;
    }
    for i in (0..6).rev() {
        image[off + i] = (value % 10) as u8;
        value /= 10;
    }
    true
}

/// DTMF code: each char mapped to its `DTMF_CHARS` index; unused cells 0xFF.
fn put_dtmf(image: &mut [u8], off: usize, len: usize, code: &str) -> bool {
    if off + len > image.len() {
        return false;
    }
    let chars: Vec<u8> = code.bytes().collect();
    for i in 0..len {
        image[off + i] = match chars.get(i) {
            Some(&c) => DTMF_CHARS
                .iter()
                .position(|&d| d == c)
                .map(|p| p as u8)
                .unwrap_or(0xFF),
            None => 0xFF,
        };
    }
    true
}

/// ASCII text (power-on message): chars then SPACE padding. The radio renders
/// the power-on banner byte-for-byte and shows 0xFF as solid blocks, so unused
/// cells must be spaces (0x20) — which is how the radio pads this field itself.
fn put_text(image: &mut [u8], off: usize, len: usize, text: &str) -> bool {
    if off + len > image.len() {
        return false;
    }
    let bytes: Vec<u8> = text.bytes().collect();
    for i in 0..len {
        image[off + i] = match bytes.get(i) {
            Some(&c) if c.is_ascii() => c,
            _ => 0x20,
        };
    }
    true
}

// ============================================================
// Decode: radio image -> profile settings (inverse of the encoder)
// ============================================================

struct FieldDef {
    ftype: String,
    options: Vec<String>,
}

/// Parse the schema into ordered (key, def) pairs so decode produces a settings
/// object covering exactly the model's editable fields, in schema order.
fn parse_fields(schema_json: &str) -> Result<Vec<(String, FieldDef)>, String> {
    let schema: Value = serde_json::from_str(schema_json)
        .map_err(|e| format!("invalid settings schema JSON: {e}"))?;
    let arr = schema.as_array().ok_or("settings schema is not a JSON array")?;
    let mut out = Vec::with_capacity(arr.len());
    for field in arr {
        let Some(key) = field.get("key").and_then(Value::as_str) else {
            continue;
        };
        let ftype = field
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("text")
            .to_string();
        let options = field
            .get("options")
            .and_then(Value::as_array)
            .map(|opts| {
                opts.iter()
                    .filter_map(|o| o.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        out.push((key.to_string(), FieldDef { ftype, options }));
    }
    Ok(out)
}

/// Decode the model's settings out of a downloaded image into a JSON object
/// keyed by CHIRP setting names, with values shaped like the profile form
/// (booleans, numbers, select labels, text). The inverse of
/// [`apply_profile_settings`]; reads only fields the schema declares and we know
/// how to locate.
pub fn decode_settings_from_image(image: &[u8], schema_json: &str) -> Result<Value, String> {
    let fields = parse_fields(schema_json)?;
    let mut map = serde_json::Map::new();
    for (key, def) in fields {
        let Some(loc) = loc(&key) else { continue };
        if let Some(v) = read_value(image, &loc, &def) {
            map.insert(key, v);
        }
    }
    Ok(Value::Object(map))
}

/// Read one setting's value from the image, formatted per the field type.
fn read_value(image: &[u8], loc: &Loc, def: &FieldDef) -> Option<Value> {
    let numeric = |n: i64| -> Option<Value> {
        match def.ftype.as_str() {
            "boolean" => Some(Value::Bool(n != 0)),
            "select" => def
                .options
                .get(n as usize)
                .map(|label| Value::String(label.clone())),
            _ => Some(Value::Number(n.into())), // integer (and any numeric fallback)
        }
    };
    match loc {
        Loc::Byte(off) => image.get(*off).and_then(|&b| numeric(b as i64)),
        Loc::Bits(off, shift, width) => {
            let b = *image.get(*off)?;
            let mask = ((1u16 << width) - 1) as u8;
            numeric(((b >> shift) & mask) as i64)
        }
        Loc::Bbcd(off) => {
            let hi = *image.get(*off)?;
            let lo = *image.get(*off + 1)?;
            let n = (hi >> 4) as i64 * 1000
                + (hi & 0x0F) as i64 * 100
                + (lo >> 4) as i64 * 10
                + (lo & 0x0F) as i64;
            numeric(n)
        }
        Loc::Freq { off, .. } => Some(Value::String(read_digits_mhz(image, *off, 8, 10))),
        Loc::Offset(off) => Some(Value::String(read_digits_mhz(image, *off, 6, 1000))),
        Loc::Dtmf(off, len) => Some(Value::String(read_dtmf(image, *off, *len))),
        Loc::Text(off, len) => Some(Value::String(read_text(image, *off, *len))),
    }
}

/// Decode `count` single-digit bytes (big-endian) into a MHz string. `div` is
/// the Hz-per-unit divisor used at encode time (10 for freq, 1000 for offset).
/// Returns "" if the field is blank (any non-decimal byte, e.g. 0xFF).
fn read_digits_mhz(image: &[u8], off: usize, count: usize, hz_per_unit: u64) -> String {
    if off + count > image.len() {
        return String::new();
    }
    let mut value: u64 = 0;
    for i in 0..count {
        let d = image[off + i];
        if d > 9 {
            return String::new(); // unprogrammed / blank
        }
        value = value * 10 + d as u64;
    }
    if value == 0 {
        return String::new();
    }
    let mhz = (value * hz_per_unit) as f64 / 1_000_000.0;
    // Trim trailing zeros while keeping the value readable (e.g. "146.52").
    let s = format!("{mhz:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

fn read_dtmf(image: &[u8], off: usize, len: usize) -> String {
    let end = (off + len).min(image.len());
    image[off..end]
        .iter()
        .filter(|&&b| b < 0x1F)
        .filter_map(|&b| DTMF_CHARS.get(b as usize).map(|&c| c as char))
        .collect()
}

fn read_text(image: &[u8], off: usize, len: usize) -> String {
    let end = (off + len).min(image.len());
    let mut s = String::new();
    for &b in &image[off..end] {
        if (0x20..=0x7E).contains(&b) {
            s.push(b as char);
        } else {
            break; // 0xFF / 0x00 terminates the message
        }
    }
    s.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = r#"[
        {"key":"squelch","type":"integer","min":0,"max":9},
        {"key":"wtled","type":"select","options":["Off","Blue","Orange","Purple"]},
        {"key":"screv","type":"select","options":["TO","CO","SE"]},
        {"key":"beep","type":"boolean"},
        {"key":"limits.vhf.lower","type":"integer"},
        {"key":"vfoa.scode","type":"select","options":["1","2","3"]}
    ]"#;

    fn blank() -> Vec<u8> {
        // Large enough to cover all aux offsets (limits end ~0x1912).
        vec![0u8; 0x1A00]
    }

    #[test]
    fn settings_land_at_absolute_chirp_offsets() {
        // Anchor against CHIRP's #seekto IMAGE offsets so the +8 ident prefix is
        // never re-added by mistake. squelch is the first byte of the settings
        // struct (#seekto 0x0E28); the menu bit is in the flag byte at 0x0E52.
        assert_eq!(SETTINGS, 0x0E28);
        let mut img = blank();
        let schema = r#"[{"key":"squelch","type":"integer"},{"key":"menu","type":"boolean"}]"#;
        apply_profile_settings(&mut img, schema, r#"{"squelch":5,"menu":true}"#).unwrap();
        assert_eq!(img[0x0E28], 5); // squelch at the struct base, not 0x0E30
        assert_eq!(img[0x0E52] & 0x01, 0x01); // menu bit at the real flag byte
    }

    #[test]
    fn integer_select_bool_encode_to_expected_bytes() {
        let mut img = blank();
        let settings = r#"{"squelch":5,"wtled":"Purple","beep":true,"vfoa.scode":"2"}"#;
        let n = apply_profile_settings(&mut img, SCHEMA, settings).unwrap();
        assert_eq!(n, 4);
        assert_eq!(img[SETTINGS], 5); // squelch
        assert_eq!(img[SETTINGS + 0x1D], 3); // wtled "Purple" -> index 3
        assert_eq!(img[SETTINGS + 0x08], 1); // beep true -> 1
        assert_eq!(img[VFOA + 0x14] & 0x0F, 1); // scode "2" -> index 1 in low nibble
    }

    #[test]
    fn bitfields_preserve_neighbor_bits() {
        let mut img = blank();
        // Pre-set the flag byte to all 1s; setting menu=false (bit0) and
        // fmradio=true (bit4) must leave the other bits untouched.
        img[SETTINGS + 0x2A] = 0xFF;
        let schema = r#"[{"key":"menu","type":"boolean"},{"key":"fmradio","type":"boolean"}]"#;
        apply_profile_settings(&mut img, schema, r#"{"menu":false,"fmradio":true}"#).unwrap();
        assert_eq!(img[SETTINGS + 0x2A] & 0x01, 0x00); // menu bit cleared
        assert_eq!(img[SETTINGS + 0x2A] & 0x10, 0x10); // fmradio bit set
        assert_eq!(img[SETTINGS + 0x2A] & 0xEE, 0xEE); // neighbors preserved
    }

    #[test]
    fn screv_writes_low_two_bits_only() {
        let mut img = blank();
        img[SETTINGS + 0x12] = 0xFC; // upper 6 bits set
        apply_profile_settings(&mut img, SCHEMA, r#"{"screv":"SE"}"#).unwrap();
        // "SE" -> index 2 in low 2 bits; upper 6 preserved.
        assert_eq!(img[SETTINGS + 0x12], 0xFE);
    }

    #[test]
    fn band_limit_encodes_big_endian_bcd() {
        let mut img = blank();
        apply_profile_settings(&mut img, SCHEMA, r#"{"limits.vhf.lower":136}"#).unwrap();
        assert_eq!(img[LIMITS + 1], 0x01);
        assert_eq!(img[LIMITS + 2], 0x36);
    }

    #[test]
    fn poweron_message_writes_chars_and_pads() {
        let mut img = blank();
        let schema = r#"[{"key":"poweron_msg.line1","type":"text"}]"#;
        apply_profile_settings(&mut img, schema, r#"{"poweron_msg.line1":"WW8L"}"#).unwrap();
        assert_eq!(&img[PONMSG..PONMSG + 4], b"WW8L");
        // Unused cells are spaces (the radio shows 0xFF as black blocks).
        assert_eq!(&img[PONMSG + 4..PONMSG + 7], b"   ");
    }

    #[test]
    fn unknown_keys_and_blank_freq_are_skipped() {
        let mut img = blank();
        let schema = r#"[{"key":"vfoa.freq","type":"text"}]"#;
        let n = apply_profile_settings(
            &mut img,
            schema,
            r#"{"vfoa.freq":"","not_a_real_key":7}"#,
        )
        .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn encode_then_decode_round_trips() {
        let schema = r#"[
            {"key":"squelch","type":"integer","min":0,"max":9},
            {"key":"wtled","type":"select","options":["Off","Blue","Orange","Purple"]},
            {"key":"beep","type":"boolean"},
            {"key":"screv","type":"select","options":["TO","CO","SE"]},
            {"key":"limits.vhf.lower","type":"integer"},
            {"key":"poweron_msg.line1","type":"text"},
            {"key":"vfoa.freq","type":"text"}
        ]"#;
        let original = r#"{"squelch":4,"wtled":"Orange","beep":false,"screv":"SE","limits.vhf.lower":144,"poweron_msg.line1":"HELLO","vfoa.freq":"446"}"#;
        let mut img = blank();
        // Start the screv byte non-zero to prove only the low 2 bits round-trip.
        img[SETTINGS + 0x12] = 0xA0;
        apply_profile_settings(&mut img, schema, original).unwrap();
        let decoded = decode_settings_from_image(&img, schema).unwrap();

        assert_eq!(decoded["squelch"], serde_json::json!(4));
        assert_eq!(decoded["wtled"], serde_json::json!("Orange"));
        assert_eq!(decoded["beep"], serde_json::json!(false));
        assert_eq!(decoded["screv"], serde_json::json!("SE"));
        assert_eq!(decoded["limits.vhf.lower"], serde_json::json!(144));
        assert_eq!(decoded["poweron_msg.line1"], serde_json::json!("HELLO"));
        assert_eq!(decoded["vfoa.freq"], serde_json::json!("446"));
    }

    #[test]
    fn vfo_frequency_encodes_digits_and_band_bit() {
        let mut img = blank();
        let schema = r#"[{"key":"vfoa.freq","type":"text"}]"#;
        apply_profile_settings(&mut img, schema, r#"{"vfoa.freq":"446.000"}"#).unwrap();
        // Hz/10 = 44600000 -> digits 4 4 6 0 0 0 0 0
        assert_eq!(&img[VFOA..VFOA + 8], &[4, 4, 6, 0, 0, 0, 0, 0]);
        assert_eq!(img[VFOA + 0x12] & 0x01, 1); // 446 MHz >= 400 -> band bit set
    }
}
