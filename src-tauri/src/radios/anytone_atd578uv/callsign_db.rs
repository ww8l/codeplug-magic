//! AT-D578UV **call-sign database** (caller ID): up to 500,000 DMR IDs with
//! name, city, callsign, state and country, shown when a station is heard.
//!
//! # Layout (qdmr's `D878UV2CallsignDB`, which the D578 uses; checked byte for
//! byte against the anchor radio's own 281,304-entry database)
//!
//! - **Index** at [`INDEX_BASE`]: banks of [`INDEX_BANK_BYTES`] (16,000 eight-byte
//!   entries) every [`BANK_STEP`]. Each entry is a key — the DMR ID in BCD as a
//!   u32, shifted left one bit, bit 0 the group-call flag (u32 LE) — and the
//!   record's byte offset in the record stream as if the banks had no gaps
//!   (u32 LE). Sorted by key, so the radio can binary-search a heard ID.
//! - **Limits** at [`LIMITS_BASE`]: entry count (u32 LE), then the end of the
//!   record stream as a REAL address (u32 LE) — gaps between banks included.
//!   qdmr writes a gap-free end against the D868's base; the anchor radio holds
//!   `0x07808232` for 281,304 entries, which is only the real-address form.
//! - **Records** at [`RECORD_BASE`]: banks of [`RECORD_BANK_BYTES`] every
//!   [`BANK_STEP`], one continuous stream that runs on across banks (a record
//!   may straddle two). Each record: call type (1: 0 private, 1 group), DMR ID
//!   (BCD BE, 4), flags (1: friend bit 4, ring tone bits 1-0; written 0), then
//!   name, city, callsign, state, country and comment, each cut to its byte
//!   limit and NUL-terminated.
//!
//! # Text
//!
//! The fields are raw single-byte text. The anchor radio's own database (written
//! by another tool) holds whatever bytes its source had — Latin-1 `0xE9` for é
//! beside UTF-8 `c3 a8` for è, and 4,037 fields cut through a multi-byte
//! character — so it settles the byte limits (names never exceed 15 bytes there,
//! not qdmr's 16) but not an encoding. Text from the app's library is folded to
//! ASCII first (`util::fold_ascii`: René -> Rene), the one encoding every AnyTone
//! font draws, and then cut to the limit.
//!
//! # Writing
//!
//! Every index and record bank sits at the start of the first 128 KB half of
//! its 256 KB block, with nothing else in that half (see the flash-half notes
//! in `mod.rs`), and no bank reaches a half's marker frame. So the database is
//! written DIRECT — no read phase, no backup — one whole bank per half, which
//! is exactly what the radio rebuilds the half from.

use crate::radios::anytone_atd890uv::{bcd_be_encode, end_session, open_port};
use crate::util::fold_ascii;
use crate::radios::driver::{CallsignDbReport, CallsignDbWriter, CallsignRecord};

use super::{enter, write_sendable, AnytoneAtd578uv};

pub(crate) const INDEX_BASE: u32 = 0x0400_0000;
pub(crate) const LIMITS_BASE: u32 = 0x0484_0000;
pub(crate) const RECORD_BASE: u32 = 0x0550_0000;
pub(crate) const BANK_STEP: u32 = 0x0004_0000;
pub(crate) const INDEX_ENTRY: usize = 8;
pub(crate) const INDEX_BANK_BYTES: usize = 16_000 * INDEX_ENTRY; // 0x1F400
pub(crate) const RECORD_BANK_BYTES: usize = 100_000; // 0x186A0
/// The radio's capacity, and what the index region holds before it reaches
/// the contact ID table at 0x04800000 (32 banks of 16,000).
pub(crate) const MAX_ENTRIES: usize = 500_000;

/// Per-field byte limits: name, city, callsign, state, country, comment. The
/// anchor radio's 281,304 records peak at exactly these lengths.
pub(crate) const FIELD_LIMITS: [usize; 6] = [15, 15, 8, 16, 16, 0];

/// One caller-ID entry. Fields are the bytes as stored.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    pub dmr_id: u32,
    pub group_call: bool,
    pub fields: [Vec<u8>; 6],
}

impl Entry {
    fn key(&self) -> u32 {
        let bcd = u32::from_be_bytes(
            bcd_be_encode(self.dmr_id as u64, 4).try_into().expect("four BCD bytes"),
        );
        (bcd << 1) | self.group_call as u32
    }

    /// The record's bytes: header, then six NUL-terminated fields cut to their
    /// byte limits (a NUL inside a field would end it early, so it is dropped).
    fn record(&self) -> Vec<u8> {
        let mut r = Vec::with_capacity(64);
        r.push(self.group_call as u8);
        r.extend_from_slice(&bcd_be_encode(self.dmr_id as u64, 4));
        r.push(0);
        for (bytes, &limit) in self.fields.iter().zip(&FIELD_LIMITS) {
            r.extend(bytes.iter().copied().filter(|&b| b != 0).take(limit));
            r.push(0);
        }
        r
    }
}

/// The encoded database: every block to write, in address order.
#[derive(Debug)]
pub(crate) struct Encoded {
    pub entries: usize,
    /// Entries left out because the radio holds [`MAX_ENTRIES`].
    pub dropped: usize,
    pub blocks: Vec<(u32, Vec<u8>)>,
}

/// Encode a database: entries sorted by key (on a duplicate key the LAST entry
/// wins — the command appends the hand-curated talkgroups after the bulk user
/// library so that they do),
/// the record stream split into banks, the index, and the limits record. Each
/// bank's block is padded to a whole 16-byte frame (index with 0xFF, records
/// with 0x00, as the radio's own database is).
pub(crate) fn encode(entries: &[Entry]) -> Result<Encoded, String> {
    // Reversed, then a stable sort: the last of each duplicate run comes first,
    // and `dedup_by_key` keeps the first of a run.
    let mut sorted: Vec<&Entry> = entries.iter().rev().collect();
    sorted.sort_by_key(|e| e.key());
    sorted.dedup_by_key(|e| e.key());
    if sorted.is_empty() {
        return Err("no entries to write".into());
    }
    // Over capacity: keep every group entry (the talkgroups the TX screen names)
    // and the users that come FIRST in the input — the library hands them over
    // in priority order — then restore key order.
    let mut dropped = 0;
    if sorted.len() > MAX_ENTRIES {
        let rank: std::collections::HashMap<u32, usize> =
            entries.iter().enumerate().map(|(i, e)| (e.key(), i)).collect();
        let groups = sorted.iter().filter(|e| e.group_call).count();
        let mut users: Vec<&Entry> = sorted.iter().copied().filter(|e| !e.group_call).collect();
        users.sort_by_key(|e| rank[&e.key()]);
        users.truncate(MAX_ENTRIES.saturating_sub(groups));
        let kept: std::collections::HashSet<u32> = users.iter().map(|e| e.key()).collect();
        let before = sorted.len();
        sorted.retain(|e| e.group_call || kept.contains(&e.key()));
        dropped = before - sorted.len();
    }

    let mut stream = Vec::new();
    let mut index = Vec::with_capacity(sorted.len() * INDEX_ENTRY);
    for e in &sorted {
        index.extend_from_slice(&e.key().to_le_bytes());
        index.extend_from_slice(&(stream.len() as u32).to_le_bytes());
        stream.extend(e.record());
    }

    let pad = |mut v: Vec<u8>, fill: u8| {
        v.resize(v.len().div_ceil(16) * 16, fill);
        v
    };
    let mut blocks = vec![];
    for (i, chunk) in index.chunks(INDEX_BANK_BYTES).enumerate() {
        blocks.push((INDEX_BASE + i as u32 * BANK_STEP, pad(chunk.to_vec(), 0xFF)));
    }
    let mut limits = vec![0u8; 16];
    limits[0..4].copy_from_slice(&(sorted.len() as u32).to_le_bytes());
    let banks = stream.len().div_ceil(RECORD_BANK_BYTES);
    let last = stream.len() - (banks - 1) * RECORD_BANK_BYTES;
    let end = RECORD_BASE + (banks as u32 - 1) * BANK_STEP + last as u32;
    limits[4..8].copy_from_slice(&end.to_le_bytes());
    blocks.push((LIMITS_BASE, limits));
    for (i, chunk) in stream.chunks(RECORD_BANK_BYTES).enumerate() {
        blocks.push((RECORD_BASE + i as u32 * BANK_STEP, pad(chunk.to_vec(), 0x00)));
    }
    blocks.sort_by_key(|(a, _)| *a);
    Ok(Encoded { entries: sorted.len(), dropped, blocks })
}

/// The app's records as entries: IDs the radio cannot hold dropped, text folded.
pub(crate) fn entries_from_records(records: &[CallsignRecord]) -> Vec<Entry> {
    records
        .iter()
        .filter(|r| r.dmr_id > 0 && r.dmr_id <= 16_777_215)
        .map(|r| Entry {
            dmr_id: r.dmr_id,
            group_call: r.group_call,
            fields: [
                fold_ascii(&r.name),
                fold_ascii(r.city.as_deref().unwrap_or_default()),
                fold_ascii(&r.callsign),
                fold_ascii(r.state.as_deref().unwrap_or_default()),
                fold_ascii(r.country.as_deref().unwrap_or_default()),
                fold_ascii(r.comment.as_deref().unwrap_or_default()),
            ],
        })
        .collect()
}

impl CallsignDbWriter for AnytoneAtd578uv {
    /// Encode and write the whole database DIRECT (see the module doc): one
    /// session, every block whole, END once.
    fn write_callsign_db(&self, port: &str, records: &[CallsignRecord]) -> Result<CallsignDbReport, String> {
        let encoded = encode(&entries_from_records(records))?;
        let mut p = open_port(port)?;
        let _ident = enter(&mut *p)?;
        for (addr, data) in &encoded.blocks {
            // A failed frame leaves the session open on purpose: END would
            // commit a half-written database.
            write_sendable(&mut *p, *addr, data)
                .map_err(|e| format!("{e} — the call-sign database is half written; power-cycle the radio and write it again"))?;
        }
        let _ = end_session(&mut *p);
        Ok(CallsignDbReport {
            entries_written: encoded.entries,
            windows_written: encoded.blocks.iter().map(|(a, _)| format!("0x{a:08X}")).collect(),
            note: if encoded.dropped > 0 {
                format!(
                    "Written and committed — the radio reboots. The radio holds {MAX_ENTRIES} \
                     entries, so the {} lowest-priority users were left out.",
                    encoded.dropped
                )
            } else {
                "Written and committed — the radio reboots. Check a heard station's name on its \
                 screen."
                    .to_string()
            },
            verified: false,
        })
    }
}

/// Decode a database back out of its blocks, for the byte-identical gate.
#[cfg(test)]
pub(crate) fn decode(blocks: &std::collections::BTreeMap<u32, Vec<u8>>) -> Vec<Entry> {
    let lim = &blocks[&LIMITS_BASE];
    let count = u32::from_le_bytes(lim[0..4].try_into().unwrap()) as usize;
    let mut stream = Vec::new();
    for bank in 0.. {
        let Some(b) = blocks.get(&(RECORD_BASE + bank * BANK_STEP)) else { break };
        stream.extend_from_slice(&b[..b.len().min(RECORD_BANK_BYTES)]);
    }
    let mut out = Vec::with_capacity(count);
    let mut pos = 0;
    for _ in 0..count {
        let r = &stream[pos..];
        let id = crate::radios::anytone_atd890uv::bcd_be(&r[1..5]) as u32;
        let mut fields: [Vec<u8>; 6] = Default::default();
        let mut o = 6;
        for f in fields.iter_mut() {
            let n = r[o..].iter().position(|&b| b == 0).unwrap();
            *f = r[o..o + n].to_vec();
            o += n + 1;
        }
        out.push(Entry { dmr_id: id, group_call: r[0] == 1, fields });
        pos += o;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u32, group: bool, name: &str) -> Entry {
        Entry {
            dmr_id: id,
            group_call: group,
            fields: [fold_ascii(name), b"City".to_vec(), b"CALL".to_vec(), b"ST".to_vec(), b"USA".to_vec(), vec![]],
        }
    }

    #[test]
    fn a_duplicate_key_keeps_the_last_entry_and_group_is_a_separate_key() {
        let enc = encode(&[
            entry(3100, false, "User"),
            entry(3100, true, "TG USA"),
            entry(3100, false, "Hand-curated"),
        ])
        .unwrap();
        assert_eq!(enc.entries, 2);
        let blocks = enc.blocks.into_iter().collect();
        let back = decode(&blocks);
        assert_eq!(back[0].fields[0], b"Hand-curated");
        assert!(!back[0].group_call);
        assert!(back[1].group_call, "the group entry sorts after the private one");
    }

    #[test]
    fn records_straddle_banks_and_the_end_is_a_real_address() {
        // 2,500 entries of 6 + (5+1)+(4+1)+(4+1)+(2+1)+(3+1)+1 = 30 bytes = 75,000
        // bytes — one bank; 4,000 such entries run into a second bank.
        let entries: Vec<Entry> = (1..=4000).map(|i| entry(1_000_000 + i, false, "Name0")).collect();
        let enc = encode(&entries).unwrap();
        let stream_len = 4000 * 30;
        let end = RECORD_BASE + BANK_STEP + (stream_len - RECORD_BANK_BYTES) as u32;
        let limits = &enc.blocks.iter().find(|(a, _)| *a == LIMITS_BASE).unwrap().1;
        assert_eq!(u32::from_le_bytes(limits[4..8].try_into().unwrap()), end);
        let blocks: std::collections::BTreeMap<u32, Vec<u8>> = enc.blocks.into_iter().collect();
        assert_eq!(blocks[&RECORD_BASE].len(), RECORD_BANK_BYTES);
        assert_eq!(decode(&blocks), entries);
    }

    #[test]
    fn over_capacity_keeps_talkgroups_and_the_highest_priority_users() {
        // Priority order is input order: the last users in the input go first.
        let mut entries: Vec<Entry> = (0..MAX_ENTRIES as u32 + 10).map(|i| entry(1_000_000 + i, false, "U")).collect();
        entries.push(entry(91, true, "Worldwide"));
        let enc = encode(&entries).unwrap();
        assert_eq!(enc.entries, MAX_ENTRIES);
        assert_eq!(enc.dropped, 11);
        let back = decode(&enc.blocks.into_iter().collect());
        assert!(back.iter().any(|e| e.group_call && e.dmr_id == 91), "the talkgroup survives");
        assert!(back.iter().any(|e| e.dmr_id == 1_000_000), "the first user survives");
        assert!(!back.iter().any(|e| e.dmr_id == 1_000_000 + MAX_ENTRIES as u32 + 9), "the last is dropped");
    }

    #[test]
    fn fields_are_cut_to_their_byte_limits() {
        let mut e = entry(1, false, "A very long name that overflows");
        e.fields[2] = b"VERYLONGCALL".to_vec();
        let enc = encode(&[e]).unwrap();
        let back = decode(&enc.blocks.into_iter().collect());
        assert_eq!(back[0].fields[0], b"A very long nam", "names stop at 15 bytes");
        assert_eq!(back[0].fields[2], b"VERYLONG");
    }

    #[test]
    fn library_text_is_folded_to_ascii() {
        assert_eq!(fold_ascii("René Liège"), b"Rene Liege");
        assert_eq!(fold_ascii("Jürgen Groß"), b"Jurgen Gross");
        assert_eq!(fold_ascii("Søren Łódź"), b"Soren Lodz");
        assert_eq!(fold_ascii("東京"), b"??");
        assert_eq!(fold_ascii("tab\there"), b"tabhere");
    }

    /// ★ The gate: decode the anchor radio's own database (a full read-only dump,
    /// kept out of the repo — it is radioid.net user data) and re-encode it. The
    /// index, every record bank and the limits record must come out byte for
    /// byte as the radio holds them.
    ///   CPM_CSDB=scratchpad/anytone_atd578uv/csdb_full.bin cargo test --lib csdb_gate -- --ignored --nocapture
    #[test]
    #[ignore = "needs CPM_CSDB pointing at a full call-sign DB dump"]
    fn csdb_gate_the_radios_database_reencodes_byte_identically() {
        let path = std::env::var("CPM_CSDB").expect("CPM_CSDB");
        let blocks: std::collections::BTreeMap<u32, Vec<u8>> =
            crate::radios::anytone_atd890uv::parse_dump(&std::fs::read(path).unwrap())
                .unwrap()
                .into_iter()
                .collect();
        let entries = decode(&blocks);
        eprintln!("decoded {} entries", entries.len());
        let enc = encode(&entries).unwrap();
        assert_eq!(enc.entries, entries.len(), "no entry was a duplicate key");
        for (addr, data) in &enc.blocks {
            let radio = &blocks[addr];
            let n = data.len().min(radio.len());
            let first = (0..n).find(|&i| data[i] != radio[i]);
            assert_eq!(first, None, "block 0x{addr:08X} differs at +0x{:X}", first.unwrap_or(0));
            assert_eq!(data.len(), radio.len(), "block 0x{addr:08X} length");
        }
        eprintln!("{} blocks byte-identical", enc.blocks.len());
    }
}
