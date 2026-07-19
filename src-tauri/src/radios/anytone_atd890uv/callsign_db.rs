//! AnyTone AT-D890UV **Call-sign Database** ("caller ID" DB) write encoder.
//!
//! This is the radio-specific consumer of the model-agnostic `dmr_users`
//! library (session 34): given a batch of DMR-ID → callsign/name records it
//! produces the [`RegionPatch`]es for the three on-radio sub-regions, ready for
//! the proven `run_patch_writes` machinery. Pure + unit-tested; no hardware.
//!
//! # On-radio layout (firmware 1.05, from the community `d890uv-v1.05.xml` RE)
//! Three cooperating regions:
//!
//! 1. **Call-sign Database Limits** @ [`LIMITS_BASE`] (16 bytes): `Entry Count`
//!    (u32 LE) then `End-of-database address` (u32 LE, absolute) then 8 unused.
//! 2. **Contact Map** @ [`MAP_BASE`], `step` [`BANK_STEP`], up to
//!    [`MAP_BANKS`] banks × [`MAP_ENTRIES_PER_BANK`] entries. Each 8-byte entry
//!    is a sorted lookup key `(bcd8be(dmr_id) << 1) | group_call_flag` (u32 LE)
//!    plus a `Contact Index` (u32 LE) into the DB banks. Sorted ascending so
//!    the radio can binary-search a received DMR ID.
//! 3. **Call-sign DB Banks** @ [`DB_BASE`], `step` [`BANK_STEP`], up to
//!    [`DB_BANKS`] banks, each capped at [`DB_BANK_CAP`] bytes.
//!    **Variable-length** records: Call Type (1) + packed flags (1) +
//!    DMR ID (4-byte big-endian BCD) + a `\0`-separated UTF-16LE description
//!    (`name\0city\0call\0state\0country\0comment`, each field NUL-terminated).
//!
//! # Decisions the empty factory unit cannot arbitrate (see module tests)
//! This unit ships empty and we never populate it via AnyTone's CPS (standing
//! project rule), so there is no populated byte image to diff against — the
//! real radio's own caller-ID screen is the arbiter. The spots the XML leaves
//! genuinely ambiguous are isolated here so a failed HW verify is a cheap flip:
//!   * **`Contact Index` = VIRTUAL gapless byte offset** (cumulative record
//!     sizes), NOT the ordinal. CORRECTED (s37) from qdmr's encoder: "the offset
//!     of the entry is not the real memory offset, but a virtual one without the
//!     gaps." The s36 2-record test couldn't tell ordinal from offset (record 0
//!     is at offset 0 either way); at scale the radio uses it to locate records
//!     across bank gaps, so it MUST be the virtual offset.
//!   * **Description = 6 UTF-16LE `\0`-terminated fields, NO trailing pad
//!     byte.** CONFIRMED (s36): the first write included the XML's nominal pad
//!     byte, which shoved every record after the first out of alignment (a
//!     read-back showed record 2's DMR ID as 31081 instead of 3108176). The
//!     parser derives record length from the six NUL-terminated fields alone.
//!     [`encode_description`] is the single place these widths live.
//!   * **Each field is TRUNCATED to a fixed per-field character limit**
//!     ([`FIELD_LIMITS`], from qdmr's proven encoder: name 16, city 15, call 8,
//!     state 16, country 16, comment 0). CONFIRMED (s37): the radio reads each
//!     field into a fixed buffer, so an over-long field (a 37-char club name)
//!     overflowed and corrupted the parse of every record after it — CPS's list
//!     stopped at ~38. Writing full 300k data with truncation fixes it.

use super::{bcd_be_encode, run_patch_writes_direct, AnytoneAtd890uv, RegionPatch};
use crate::radios::driver::{CallsignDbWriter, CallsignRecord};

/// Call-sign Database Limits record (Entry Count + End-of-DB address + 8 pad).
pub const LIMITS_BASE: u32 = 0x0700_0000;
/// First Contact Map bank.
pub const MAP_BASE: u32 = 0x0708_0000;
/// First Call-sign DB bank.
pub const DB_BASE: u32 = 0x0790_0000;
/// Address stride between successive banks in both the map and DB regions.
pub const BANK_STEP: u32 = 0x0008_0000;

/// Max Contact Map banks (`repeat max="16"`).
pub const MAP_BANKS: usize = 16;
/// Contact Map entries per bank (`repeat max="32000"`); each entry is 8 bytes.
pub const MAP_ENTRIES_PER_BANK: usize = 32_000;
/// One Contact Map entry: `key` (u32 LE) + `contact_index` (u32 LE).
pub const MAP_ENTRY_LEN: usize = 8;

/// Max Call-sign DB banks (`repeat max="500"`).
pub const DB_BANKS: usize = 500;
/// Per-bank byte cap (`0x30d40` = 200000, from the XML brief).
pub const DB_BANK_CAP: usize = 0x0003_0D40;

/// One caller-ID record to program. Text fields are already human-readable;
/// [`encode_callsign_db`] packs them into the radio's on-wire layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallsignDbEntry {
    /// DMR ID, 1..=99_999_999 (8 BCD digits).
    pub dmr_id: u32,
    /// Person/display name (description field 1).
    pub name: String,
    /// City (field 2).
    pub city: String,
    /// Callsign (field 3, "call").
    pub call: String,
    /// State/province (field 4).
    pub state: String,
    /// Country (field 5).
    pub country: String,
    /// Free-form comment (field 6).
    pub comment: String,
}

/// Contact Map lookup key for a private-call caller-ID record:
/// `(bcd8be(dmr_id) << 1) | group_call_flag`. `bcd8be` is the DMR ID as a
/// 4-byte big-endian BCD value read back as a u32 (e.g. id 3112345 →
/// bytes 03 11 23 45 → 0x03112345). Group-call flag is 0 for private calls.
fn map_key(dmr_id: u32) -> u32 {
    let bcd = bcd_be_encode(dmr_id as u64, 4);
    let bcd8be = u32::from_be_bytes([bcd[0], bcd[1], bcd[2], bcd[3]]);
    bcd8be << 1
}

/// Per-field maximum lengths, in characters, exactly as qdmr's D868/D890
/// call-sign DB encoder enforces them (`d868uv_callsigndb`: `nameLength`=16,
/// `cityLength`=15, `callLength`=8, `stateLength`=16, `countryLength`=16,
/// `commentLength`=0). The radio reads each field into a FIXED-size buffer, so a
/// field longer than its limit overflows and corrupts the parse of every record
/// after it — HW-proven: a 37-char name ("Canadian Caribbean Amateur Radio
/// Club") derailed CPS's list at that record while every shorter record before
/// it read fine. Order: name, city, call, state, country, comment.
const FIELD_LIMITS: [usize; 6] = [16, 15, 8, 16, 16, 0];

/// Encode one description = 6 UTF-16LE fields, each **truncated to its
/// [`FIELD_LIMITS`] character cap** and terminated by a `\0` (0x0000) code unit,
/// in the fixed order name, city, call, state, country, comment. There is NO
/// trailing pad byte: the parser computes a record's length by consuming exactly
/// these six NUL-terminated fields (confirmed via an AnyTone-CPS read-back, s36),
/// so an extra byte pushes the next record out of alignment. Truncation is by
/// Unicode scalar (`chars`) so a multi-byte glyph is never split.
fn encode_description(e: &CallsignDbEntry) -> Vec<u8> {
    let fields = [&e.name, &e.city, &e.call, &e.state, &e.country, &e.comment];
    let mut out = Vec::new();
    for (field, &limit) in fields.iter().zip(FIELD_LIMITS.iter()) {
        for c in field.chars().take(limit) {
            let mut buf = [0u16; 2];
            for u in c.encode_utf16(&mut buf) {
                out.extend_from_slice(&u.to_le_bytes());
            }
        }
        out.extend_from_slice(&[0x00, 0x00]); // UTF-16 NUL separator/terminator
    }
    out
}

/// One Call-sign DB record: Call Type (0 = Private) + packed flags (0) +
/// DMR ID (4-byte BE BCD) + description.
fn encode_db_record(e: &CallsignDbEntry) -> Vec<u8> {
    let mut rec = Vec::with_capacity(6 + e.name.len() * 2 + 16);
    rec.push(0x00); // Call Type: Private Call
    rec.push(0x00); // packed flags: Friend=0, Ring Tone=0
    rec.extend_from_slice(&bcd_be_encode(e.dmr_id as u64, 4));
    rec.extend_from_slice(&encode_description(e));
    rec
}

/// Build the [`RegionPatch`]es for a caller-ID batch across all three regions.
///
/// The input is deduplicated and sorted ascending by DMR ID; the map and DB
/// banks are written in that same order. Each map entry's `contact_index` is the
/// record's VIRTUAL gapless byte offset (cumulative record sizes), not its
/// ordinal. The DB region is packed as EXACTLY-`DB_BANK_CAP`-byte physical banks
/// at [`BANK_STEP`] stride, records splitting across bank boundaries (banks must
/// be completely full or the radio's reader stops at the first 0xFF slack). The
/// map region packs 32000 fixed 8-byte entries per bank.
///
/// Errors on: empty input, an out-of-range DMR ID, or overflow of the map/DB
/// bank capacity. Pure; no hardware.
pub fn encode_callsign_db(entries: &[CallsignDbEntry]) -> Result<Vec<RegionPatch>, String> {
    if entries.is_empty() {
        return Err("no call-sign DB entries supplied".to_string());
    }

    // Sort ascending by DMR ID and drop duplicate IDs (last wins), so the map
    // is binary-searchable and DB ordinals line up with map contact indices.
    let mut sorted: Vec<CallsignDbEntry> = entries.to_vec();
    sorted.sort_by_key(|e| e.dmr_id);
    sorted.dedup_by_key(|e| e.dmr_id);

    for e in &sorted {
        if e.dmr_id == 0 || e.dmr_id > 99_999_999 {
            return Err(format!("DMR ID {} out of range (1..=99_999_999)", e.dmr_id));
        }
    }
    if sorted.len() > MAP_BANKS * MAP_ENTRIES_PER_BANK {
        return Err(format!(
            "{} entries exceed contact-map capacity ({})",
            sorted.len(),
            MAP_BANKS * MAP_ENTRIES_PER_BANK
        ));
    }

    let mut patches = Vec::new();

    // Encode every record (DMR-ID sorted) and record each one's VIRTUAL offset —
    // the cumulative byte position in a GAPLESS concatenation of all records.
    // This virtual offset (not the ordinal, not the physical address) is what the
    // Contact Map stores as each entry's `contact_index`: qdmr's comment is
    // explicit — "the offset of the entry is not the real memory offset, but a
    // virtual one without the gaps." The radio converts it back to a physical
    // address by dividing/modding by the per-bank cap.
    let records: Vec<Vec<u8>> = sorted.iter().map(encode_db_record).collect();
    let mut virtual_offsets: Vec<u32> = Vec::with_capacity(records.len());
    let mut acc: u64 = 0;
    for r in &records {
        virtual_offsets.push(acc as u32);
        acc += r.len() as u64;
    }

    // --- Region 3: Call-sign DB Banks. Concatenate all records into one gapless
    //     blob, then slice it into EXACTLY-`DB_BANK_CAP`-byte physical banks at
    //     `BANK_STEP` stride. Records are SPLIT across bank boundaries so each
    //     bank (except the last) is packed completely full — leaving 0xFF slack
    //     at a bank's end makes the radio's reader stop there (HW-proven: the
    //     list stopped at ~1891, exactly where bank 0's data used to run out). ---
    let blob: Vec<u8> = records.concat();
    let mut end_of_db = DB_BASE;
    let mut off = 0usize;
    let mut bank_idx = 0usize;
    while off < blob.len() {
        if bank_idx >= DB_BANKS {
            return Err(format!("DB banks overflow (>{DB_BANKS} banks)"));
        }
        let take = (blob.len() - off).min(DB_BANK_CAP);
        let base = DB_BASE + (bank_idx as u32) * BANK_STEP;
        end_of_db = base + take as u32;
        patches.push(RegionPatch { addr: base, data: blob[off..off + take].to_vec() });
        off += take;
        bank_idx += 1;
    }

    // --- Region 2: Contact Map (sorted by key; contact_index = virtual offset).
    //     Fixed 8-byte entries, 32000 per 0x80000 bank. ---
    let mut map_bank: Vec<u8> = Vec::new();
    let mut map_bank_idx = 0usize;
    for (i, e) in sorted.iter().enumerate() {
        if i > 0 && i % MAP_ENTRIES_PER_BANK == 0 {
            let base = MAP_BASE + (map_bank_idx as u32) * BANK_STEP;
            patches.push(RegionPatch { addr: base, data: std::mem::take(&mut map_bank) });
            map_bank_idx += 1;
        }
        map_bank.extend_from_slice(&map_key(e.dmr_id).to_le_bytes());
        map_bank.extend_from_slice(&virtual_offsets[i].to_le_bytes());
    }
    if !map_bank.is_empty() {
        let base = MAP_BASE + (map_bank_idx as u32) * BANK_STEP;
        patches.push(RegionPatch { addr: base, data: map_bank });
    }

    // --- Region 1: Limits (entry count + absolute end-of-DB address). Write
    //     only the 8 meaningful bytes; the 8 "unused" bytes that follow are
    //     left exactly as the radio holds them (0xFF on a factory unit — the
    //     baseline read confirmed `00 00 00 00 FF FF FF FF FF...` when empty). ---
    let mut limits = Vec::with_capacity(8);
    limits.extend_from_slice(&(sorted.len() as u32).to_le_bytes());
    limits.extend_from_slice(&end_of_db.to_le_bytes());
    patches.push(RegionPatch { addr: LIMITS_BASE, data: limits });

    Ok(patches)
}


impl CallsignDbWriter for AnytoneAtd890uv {
    /// Encode `records` into the three caller-ID regions and write them via the
    /// DIRECT (no read-modify-write) path — the RMW read phase corrupts large
    /// flash commits, and the DB region is fully overwritten so nothing needs
    /// preserving (see `run_patch_writes_direct`). Returns the entry count written.
    fn write_callsign_db(&self, port: &str, records: &[CallsignRecord]) -> Result<usize, String> {
        let entries: Vec<CallsignDbEntry> = records
            .iter()
            .filter(|r| r.dmr_id > 0 && r.dmr_id <= 99_999_999)
            .map(|r| CallsignDbEntry {
                dmr_id: r.dmr_id,
                name: r.name.clone(),
                city: r.city.clone().unwrap_or_default(),
                call: r.callsign.clone(),
                state: r.state.clone().unwrap_or_default(),
                country: r.country.clone().unwrap_or_default(),
                comment: String::new(),
            })
            .collect();
        if entries.is_empty() {
            return Err("no valid DMR IDs in the batch".to_string());
        }
        let count = entries.len();
        let patches = encode_callsign_db(&entries)?;
        run_patch_writes_direct(port, &patches)?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(dmr_id: u32, name: &str, call: &str) -> CallsignDbEntry {
        CallsignDbEntry {
            dmr_id,
            name: name.to_string(),
            city: String::new(),
            call: call.to_string(),
            state: String::new(),
            country: String::new(),
            comment: String::new(),
        }
    }

    /// Find the emitted patch at exactly `addr`.
    fn at(patches: &[RegionPatch], addr: u32) -> &RegionPatch {
        patches.iter().find(|p| p.addr == addr).expect("patch at addr")
    }

    #[test]
    fn empty_input_errors() {
        assert!(encode_callsign_db(&[]).is_err());
    }

    #[test]
    fn out_of_range_dmr_id_errors() {
        assert!(encode_callsign_db(&[entry(0, "A", "A")]).is_err());
        assert!(encode_callsign_db(&[entry(100_000_000, "A", "A")]).is_err());
    }

    #[test]
    fn map_key_is_bcd8be_shifted() {
        // 3112345 -> BE BCD 03 11 23 45 -> 0x03112345 -> <<1.
        assert_eq!(map_key(3_112_345), 0x0311_2345 << 1);
        assert_eq!(map_key(1), 0x0000_0001 << 1);
    }

    #[test]
    fn limits_region_has_count_and_end_address() {
        let patches = encode_callsign_db(&[entry(1, "A", "A")]).unwrap();
        let limits = at(&patches, LIMITS_BASE);
        assert_eq!(limits.data.len(), 8, "only the 2 meaningful u32s are written");
        assert_eq!(u32::from_le_bytes(limits.data[0..4].try_into().unwrap()), 1);
        let end = u32::from_le_bytes(limits.data[4..8].try_into().unwrap());
        // One record in bank 0: end = DB_BASE + record length.
        let rec_len = encode_db_record(&entry(1, "A", "A")).len();
        assert_eq!(end, DB_BASE + rec_len as u32);
    }

    #[test]
    fn entries_are_sorted_and_map_indexes_virtual_offsets() {
        // Supplied out of order; map must be ascending by key, and each
        // contact_index must be the record's VIRTUAL (gapless) byte offset —
        // cumulative record sizes, not the ordinal.
        let patches = encode_callsign_db(&[
            entry(500, "E", "E"),
            entry(100, "A", "A"),
            entry(300, "C", "C"),
        ])
        .unwrap();
        let map = at(&patches, MAP_BASE);
        assert_eq!(map.data.len(), 3 * MAP_ENTRY_LEN);
        let keys: Vec<u32> = (0..3)
            .map(|i| u32::from_le_bytes(map.data[i * 8..i * 8 + 4].try_into().unwrap()))
            .collect();
        let idxs: Vec<u32> = (0..3)
            .map(|i| u32::from_le_bytes(map.data[i * 8 + 4..i * 8 + 8].try_into().unwrap()))
            .collect();
        assert_eq!(keys, vec![map_key(100), map_key(300), map_key(500)]);
        // Records are DMR-ID sorted: A(100), C(300), E(500). Offsets are cumulative.
        let la = encode_db_record(&entry(100, "A", "A")).len() as u32;
        let lc = encode_db_record(&entry(300, "C", "C")).len() as u32;
        assert_eq!(idxs, vec![0, la, la + lc]);
    }

    #[test]
    fn duplicate_dmr_ids_collapse() {
        let patches =
            encode_callsign_db(&[entry(100, "old", "A"), entry(100, "new", "A")]).unwrap();
        let limits = at(&patches, LIMITS_BASE);
        assert_eq!(u32::from_le_bytes(limits.data[0..4].try_into().unwrap()), 1);
        let map = at(&patches, MAP_BASE);
        assert_eq!(map.data.len(), MAP_ENTRY_LEN);
    }

    #[test]
    fn db_record_layout_call_type_flags_bcd_and_utf16_description() {
        let e = CallsignDbEntry {
            dmr_id: 3_112_345,
            name: "Tim".to_string(),
            city: "Denver".to_string(),
            call: "W0CPH".to_string(),
            state: "CO".to_string(),
            country: "USA".to_string(),
            comment: String::new(),
        };
        let rec = encode_db_record(&e);
        assert_eq!(rec[0], 0x00, "call type private");
        assert_eq!(rec[1], 0x00, "flags");
        assert_eq!(&rec[2..6], &[0x03, 0x11, 0x23, 0x45], "BE BCD DMR ID");
        // Description begins with UTF-16LE "Tim" then a NUL separator.
        assert_eq!(&rec[6..12], &[b'T', 0, b'i', 0, b'm', 0]);
        assert_eq!(&rec[12..14], &[0x00, 0x00], "field separator");
        // Ends on the comment field's NUL terminator, with NO extra pad byte
        // (the extra byte was the s36 alignment bug). Length = 6 header + every
        // field's UTF-16 bytes + a 2-byte terminator each; nothing more.
        let field_bytes: usize = ["Tim", "Denver", "W0CPH", "CO", "USA", ""]
            .iter()
            .map(|f| f.encode_utf16().count() * 2 + 2)
            .sum();
        assert_eq!(rec.len(), 6 + field_bytes, "record length is exactly the parsed content");
        assert_eq!(&rec[rec.len() - 2..], &[0x00, 0x00], "comment terminator, no pad");
        // Six fields' worth of NUL terminators (0x0000) are present.
        let seps = rec[6..].chunks_exact(2).filter(|c| c == &[0x00, 0x00]).count();
        assert_eq!(seps, 6);
    }

    #[test]
    fn long_fields_are_truncated_to_radio_limits() {
        // A name well over the 16-char limit (the "Canadian Caribbean Amateur
        // Radio Club" case that derailed the radio's parser) must be cut to 16
        // UTF-16 units so it can't overflow the radio's fixed name buffer.
        let e = CallsignDbEntry {
            dmr_id: 3_112_345,
            name: "Canadian Caribbean Amateur Radio Club".to_string(), // 37 chars
            city: "Bowmanville".to_string(),                            // 11, under 15
            call: "VE3BRA".to_string(),                                 // 6, under 8
            state: "Ontario".to_string(),
            country: "Canada".to_string(),
            comment: "should be dropped".to_string(), // commentLength = 0
        };
        let rec = encode_db_record(&e);
        // Walk the six NUL-terminated UTF-16 fields back out.
        let mut fields: Vec<String> = Vec::new();
        let mut i = 6;
        for _ in 0..6 {
            let mut units = Vec::new();
            while rec[i] != 0 || rec[i + 1] != 0 {
                units.push(u16::from_le_bytes([rec[i], rec[i + 1]]));
                i += 2;
            }
            i += 2; // skip terminator
            fields.push(String::from_utf16(&units).unwrap());
        }
        assert_eq!(fields[0], "Canadian Caribbe", "name cut to 16 chars");
        assert_eq!(fields[0].chars().count(), 16);
        assert_eq!(fields[1], "Bowmanville", "short city untouched");
        assert_eq!(fields[2], "VE3BRA");
        assert_eq!(fields[5], "", "comment always dropped (limit 0)");
    }

    #[test]
    fn map_bank_stepping_splits_at_32000() {
        // 32001 entries -> bank 0 full (32000) + bank 1 (1 entry).
        let entries: Vec<CallsignDbEntry> =
            (1..=MAP_ENTRIES_PER_BANK as u32 + 1).map(|id| entry(id, "n", "c")).collect();
        let patches = encode_callsign_db(&entries).unwrap();
        let bank0 = at(&patches, MAP_BASE);
        let bank1 = at(&patches, MAP_BASE + BANK_STEP);
        assert_eq!(bank0.data.len(), MAP_ENTRIES_PER_BANK * MAP_ENTRY_LEN);
        assert_eq!(bank1.data.len(), MAP_ENTRY_LEN);
        // The lone bank-1 entry (record #32000) indexes the VIRTUAL offset =
        // 32000 * record-size (every record here is the same size).
        let rec_len = encode_db_record(&entry(1, "n", "c")).len() as u32;
        assert_eq!(
            u32::from_le_bytes(bank1.data[4..8].try_into().unwrap()),
            32_000 * rec_len
        );
    }

    #[test]
    fn db_bank_stepping_respects_cap() {
        // Fields are now capped per-radio (~160 bytes/record max), so force a
        // second DB bank with MANY max-size records instead of a few huge ones:
        // DB_BANK_CAP / ~160 ≈ 1250 records per bank, so 1400 spills into bank 1.
        let entries: Vec<CallsignDbEntry> = (1..=1400)
            .map(|id| CallsignDbEntry {
                dmr_id: id,
                name: "N".repeat(16),
                city: "C".repeat(15),
                call: "K".repeat(8),
                state: "S".repeat(16),
                country: "U".repeat(16),
                comment: String::new(),
            })
            .collect();
        let patches = encode_callsign_db(&entries).unwrap();
        // DB-bank patches are the ones at/above DB_BASE (map+limits sit lower).
        let db_patches: Vec<&RegionPatch> =
            patches.iter().filter(|p| p.addr >= DB_BASE).collect();
        assert!(db_patches.len() >= 2, "expected multiple DB banks");
        // Bank 0 must be packed EXACTLY full (records split across the boundary);
        // leaving slack is the bug that made the radio stop at ~1891.
        let bank0 = db_patches.iter().find(|p| p.addr == DB_BASE).unwrap();
        assert_eq!(bank0.data.len(), DB_BANK_CAP, "bank 0 packed exactly full");
        assert!(db_patches.iter().any(|p| p.addr == DB_BASE + BANK_STEP));
        for p in &db_patches {
            assert!(p.data.len() <= DB_BANK_CAP, "bank within cap");
        }
    }
}
