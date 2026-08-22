//! Direct AnyTone AT-D890UV programming over the radio's USB-C port.
//!
//! The D890UV (like the rest of the AnyTone D868/D878/D578 family) enumerates as
//! a USB CDC-ACM serial port and speaks a documented clone protocol. This module
//! is a faithful Rust implementation of that protocol so Codeplug Magic can talk to the
//! radio without Anytone CPS.
//!
//! **Stage 1 (this file): read-only.** Radio identify and a memory-map *probe*
//! that reads a handful of small, known AnyTone codeplug regions and reports
//! which responded (plus a hex preview), saving whatever was captured as a
//! timestamped backup. There is intentionally NO write/upload path yet — writing
//! the radio can brick it, so the read path is proven against real hardware first
//! (see PROJECT_STATE / the plan). Port enumeration reuses
//! `program::list_serial_ports` (it is model-agnostic).
//!
//! Protocol notes (documented by the reald/anytone-flash-tools project and the
//! qdmr `anytone_interface` transport; the wire format is shared across the
//! D868/D878/D578/D890 family):
//!   - serial: 8-N-1; baud is driver-set and effectively ignored (CDC-ACM).
//!   - enter program mode: send ASCII `PROGRAM`; radio replies `QX\x06`
//!     (`51 58 06`) and shows "PC Mode".
//!   - identify: send `0x02`; radio replies a ~16-byte ident terminated by
//!     `0x06`, ASCII like `ID890UV\0\0V100\0\0\0`. The leading `IDxxxxxx` token
//!     names the model — we require it to contain "890".
//!   - read block: send `R`(0x52) + addr(4 bytes, BIG-endian) + size(1); reply is
//!     `W`(0x57) + addr(4) + size(1) + `size` data bytes + checksum(1) + ack(0x06).
//!     The checksum is a 1-byte sum of the 4 address bytes, the length byte, and
//!     the data bytes (hardware-confirmed as `addr+len+data`).
//!   - end session: send ASCII `END`; radio replies `0x06`.
//!
//! LICENCE NOTE on the upstream citations in this header. `reald/anytone-flash-tools`
//! is bare GPL-2.0-only with no "or any later version" grant, so none of its code
//! may be combined into this GPLv3 work — and none of it is here. What came from it
//! is the fact list above (the `PROGRAM` handshake, the `QX\x06` reply, the `R`/`W`
//! frame layout, `END`), read out of its `at-d878uv_protocol.md`. How a device
//! behaves on the wire is not copyrightable subject matter. Do not copy code, tables
//! or data from that project into this tree. `qdmr` is GPLv3 and compatible; see the
//! Licence section of the README for the full accounting.
//!
//! NOTE: AnyTone codeplug memory is SPARSE and lives at high addresses (channels,
//! zones, bitmaps … all above ~0x00800000); address 0 is unmapped. The exact
//! D890UV map is not yet published, so Stage 1 probes the well-known D868/D878
//! addresses as a hypothesis and reports per-region results. Decoding the
//! captured regions into channels/zones/scan lists is Stage 2, once the offsets
//! are confirmed against these real captures.

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::error::MapErrString;
// Driver protocol (transport, encode/decode, planning) lives in the radio
// module (Chunk 3.5). Re-exported so existing `commands::anytone::*` paths
// (import.rs, the RE example binaries) keep resolving until 3.6 rewires them.
pub use crate::radios::anytone_atd890uv::*;

// The identify handshake now lives on the driver (`RadioDriver::identify`) and
// is reached through the registry-dispatched `program::identify_radio` command
// (Chunk 3.6c) — `identify_anytone` was retired with it.

#[derive(Serialize)]
pub struct AnytoneDownloadResult {
    pub ident_hex: String,
    pub ident_ascii: String,
    /// Per-region probe results.
    pub regions: Vec<RegionProbe>,
    /// Total bytes captured across all regions that responded.
    pub image_bytes: usize,
    /// Absolute path of the saved backup `.img` (concatenated region bytes), or
    /// null if nothing responded.
    pub backup_path: Option<String>,
    /// Channels decoded from the channel banks that were read (Stage 2). Empty if
    /// the bank regions didn't respond.
    pub channels: Vec<AnytoneDecodedChannel>,
    /// Zones decoded from the zone-list region(s) read, with member channels
    /// resolved to names. (Currently only zone 1 is probed.)
    pub zones: Vec<AnytoneDecodedZone>,
    /// DMR contacts (talkgroups) referenced by the decoded channels, sorted by
    /// contact index. Feeds the talkgroup side of the DB import.
    pub contacts: Vec<AnytoneDecodedContact>,
}

/// Probe the known AnyTone regions and save whatever responded. This is the
/// non-destructive proof that reading works and the data capture that Stage 2
/// decoding is reverse-engineered against. No write path is involved.
#[tauri::command]
pub async fn download_anytone_image(
    app: AppHandle,
    port: String,
) -> Result<AnytoneDownloadResult, String> {
    let backup_dir = app.path().app_data_dir().estr()?.join("radio-backups");
    std::fs::create_dir_all(&backup_dir).estr()?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup_path = backup_dir.join(format!("anytone-{stamp}.img"));

    tauri::async_runtime::spawn_blocking(move || {
        let mut p = open_port(&port)?;
        let ident = enter_program_and_ident(&mut *p)?;

        let mut regions = Vec::new();
        let mut image = Vec::new();
        for &(name, addr, len) in PROBE_REGIONS {
            let (probe, data) = probe_region(&mut *p, name, addr, len);
            image.extend_from_slice(&data);
            regions.push(probe);
        }

        // Read every channel bank so decode covers all 4000 slots, not just 0-255.
        let (bank_regions, bank_data) = read_channel_banks(&mut *p);
        for (_, data) in &bank_data {
            image.extend_from_slice(data);
        }
        regions.extend(bank_regions);

        // Decode channels first so zone members resolve to channel names.
        let banks: Vec<(usize, &[u8])> = bank_data
            .iter()
            .map(|(base, d)| (*base, d.as_slice()))
            .collect();
        let (mut channels, slot_names) = decode_channels(&banks);

        // Resolve DMR talkgroup names from the Contact bank and patch the channels.
        let (contact_regions, contact_image, contact_map) = read_contacts(&mut *p, &channels);
        image.extend_from_slice(&contact_image);
        regions.extend(contact_regions);
        for ch in &mut channels {
            if let Some(idx) = ch.contact_index {
                ch.contact_name = contact_map.get(&idx).map(|c| c.name.clone());
            }
        }
        let mut contacts: Vec<AnytoneDecodedContact> = contact_map.into_values().collect();
        contacts.sort_unstable_by_key(|c| c.index);

        // Read every populated zone (name + channel list), resolving members.
        let (zone_regions, zone_image, zones) = read_zones(&mut *p, &slot_names);
        image.extend_from_slice(&zone_image);
        regions.extend(zone_regions);
        let _ = end_session(&mut *p);

        let backup_path = if image.is_empty() {
            None
        } else {
            std::fs::write(&backup_path, &image)
                .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;
            Some(backup_path.to_string_lossy().to_string())
        };

        Ok(AnytoneDownloadResult {
            ident_hex: hex(&ident),
            ident_ascii: ascii(&ident),
            image_bytes: image.len(),
            regions,
            backup_path,
            channels,
            zones,
            contacts,
        })
    })
    .await
    .estr()?
}

/// Outcome of the Stage 3 round-trip no-op write.
#[derive(Serialize)]
pub struct AnytoneRoundtripResult {
    /// 1-based CPS slot that was round-tripped.
    pub slot: usize,
    /// Record base address, hex (e.g. "0x01003B80").
    pub addr: String,
    /// The 128-byte record as read before the write, hex.
    pub original_hex: String,
    /// The 128-byte record as read back after the write, hex.
    pub readback_hex: String,
    /// True iff the read-back bytes are identical to the original — the pass/fail
    /// signal that the write transport + checksum are correct.
    pub matched: bool,
    /// Absolute path of the 128-byte pre-write backup (mandatory safety net).
    pub backup_path: String,
    /// The original record decoded, for a human-readable "this is the channel we
    /// round-tripped" confirmation.
    pub channel: Option<AnytoneDecodedChannel>,
}

/// Outcome of the committing whole-bank no-op test.
#[derive(Serialize)]
pub struct AnytoneBankNoopResult {
    /// 1-based slot whose bank was rewritten.
    pub slot: usize,
    /// Bank base address, hex.
    pub bank_addr: String,
    /// Bank length in bytes.
    pub bank_len: usize,
    /// Absolute path of the pre-write whole-bank backup (safety net).
    pub backup_path: String,
    /// Hex preview of the first bytes of the bank (for eyeballing).
    pub preview: String,
}

/// One-session DMR time-slot flip + commit: read the record, flip `CH_TIME_SLOT`
/// (TS1↔TS2), write it back via whole-bank RMW, END (commit + reboot) — all in a
/// SINGLE program-mode session (no reboot between read and write). Returns the
/// ORIGINAL record (for backup/restore) and the old/new time-slot values. Errors
/// if the byte isn't 0/1 (not a DMR channel).
pub fn flip_timeslot_commit(port: &str, slot: usize) -> Result<(Vec<u8>, u8, u8), String> {
    let max_slot = NUM_BANKS * CH_PER_BANK;
    if slot == 0 || slot > max_slot {
        return Err(format!("slot {slot} out of range (1..={max_slot})"));
    }
    let mut p = open_port(port)?;
    let _ident = enter_program_and_ident(&mut *p)?;
    let original = read_record(&mut *p, slot_addr(slot))?;
    let old = original[CH_TIME_SLOT];
    if old > 1 {
        let _ = end_session(&mut *p);
        return Err(format!(
            "slot {slot} time-slot byte is 0x{old:02X}, not 0/1 — not a DMR channel?"
        ));
    }
    let new = 1 - old;
    let mut modified = original.clone();
    modified[CH_TIME_SLOT] = new;
    write_slot_record(&mut *p, slot, &modified)?;
    let _ = end_session(&mut *p);
    Ok((original, old, new))
}

/// Read-only: fresh-session read of one 128-byte channel record for `slot`.
pub fn read_record_for_slot(port: &str, slot: usize) -> Result<Vec<u8>, String> {
    let max_slot = NUM_BANKS * CH_PER_BANK;
    if slot == 0 || slot > max_slot {
        return Err(format!("slot {slot} out of range (1..={max_slot})"));
    }
    let mut p = open_port(port)?;
    let _ident = enter_program_and_ident(&mut *p)?;
    let rec = read_record(&mut *p, slot_addr(slot))?;
    let _ = end_session(&mut *p);
    Ok(rec)
}

/// Write one 128-byte `record` to `slot` via whole-bank RMW, then END (commit +
/// reboot). Brick-safe granularity; the single shared write path for a CLI driver.
pub fn write_record_to_slot(port: &str, slot: usize, record: &[u8]) -> Result<(), String> {
    let max_slot = NUM_BANKS * CH_PER_BANK;
    if slot == 0 || slot > max_slot {
        return Err(format!("slot {slot} out of range (1..={max_slot})"));
    }
    let mut p = open_port(port)?;
    let _ident = enter_program_and_ident(&mut *p)?;
    write_slot_record(&mut *p, slot, record)?;
    let _ = end_session(&mut *p);
    Ok(())
}

/// Read-only: fresh-session read of the entire channel bank containing `slot`.
/// For verifying a prior write persisted / left neighbours intact without writing.
pub fn read_bank_for_slot(port: &str, slot: usize) -> Result<Vec<u8>, String> {
    let max_slot = NUM_BANKS * CH_PER_BANK;
    if slot == 0 || slot > max_slot {
        return Err(format!("slot {slot} out of range (1..={max_slot})"));
    }
    let (base, bank_len, _) = bank_of_slot(slot);
    let mut p = open_port(port)?;
    let _ident = enter_program_and_ident(&mut *p)?;
    let bank = read_block(&mut *p, base, bank_len)?;
    let _ = end_session(&mut *p);
    Ok(bank)
}

/// Read-only: strict-checksum reads of several arbitrary `(addr, len)` windows in
/// ONE PC-mode session (one END/reboot total), for CLI ground-truth capture and
/// write verification outside the channel banks.
pub fn read_windows_raw(port: &str, windows: &[(u32, u32)]) -> Result<Vec<(u32, Vec<u8>)>, String> {
    let mut p = open_port(port)?;
    let _ident = enter_program_and_ident(&mut *p)?;
    let mut out = Vec::new();
    for &(addr, len) in windows {
        match read_block(&mut *p, addr, len as usize) {
            Ok(data) => out.push((addr, data)),
            Err(e) => {
                let _ = end_session(&mut *p);
                return Err(format!("read 0x{addr:08X}+0x{len:X} failed: {e}"));
            }
        }
    }
    let _ = end_session(&mut *p);
    Ok(out)
}

/// Blocking runner for the committing whole-bank no-op — the single, shared
/// implementation the Tauri command delegates to, with the backup path passed in
/// explicitly so it can also be driven from a CLI/example binary against real
/// hardware. Read the bank, back it up, write the identical bytes back, END
/// (commit + reboot). Brick-capable I/O; range-checks the slot first.
pub fn run_noop_bank_test(
    port: &str,
    slot: usize,
    backup_path: &std::path::Path,
) -> Result<AnytoneBankNoopResult, String> {
    let max_slot = NUM_BANKS * CH_PER_BANK;
    if slot == 0 || slot > max_slot {
        return Err(format!("slot {slot} out of range (1..={max_slot})"));
    }
    let (base, bank_len, _) = bank_of_slot(slot);

    let mut p = open_port(port)?;
    let _ident = enter_program_and_ident(&mut *p)?;

    // Read the whole bank and back it up BEFORE any write.
    let bank = read_block(&mut *p, base, bank_len)?;
    std::fs::write(backup_path, &bank)
        .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

    // Write the identical bytes back, then commit (END reboots the radio).
    write_block(&mut *p, base, &bank)?;
    let _ = end_session(&mut *p);

    Ok(AnytoneBankNoopResult {
        slot,
        bank_addr: format!("0x{base:08X}"),
        bank_len,
        preview: hex(&bank[..bank.len().min(32)]),
        backup_path: backup_path.to_string_lossy().to_string(),
    })
}

// ------------------------------------------------------------
// Stage 3 PERSISTENCE test (write → commit-on-reboot → fresh read → restore)
//
// The D890UV appears to buffer writes and only commit them to flash when it
// leaves PC mode (it reboots on END), while in-session reads come from flash — so
// a written change is NOT visible on a same-session read-back (this is why the
// self-restoring edit test could never confirm a change). The only way to prove a
// write persisted is to write, exit (commit + reboot), then RE-READ in a fresh
// session. Because the radio drops off USB and re-enumerates across the reboot,
// this is a deliberate user-driven 3-step flow rather than one command:
//   1. `write_timeslot_anytone` — back up, flip the time-slot byte, write, exit.
//   2. (radio reboots; user rescans/reselects the port)
//   3. `read_slot_anytone` — fresh read; user confirms the new time slot stuck.
//   4. `restore_slot_anytone` — write the backed-up original bytes back, exit.
// ------------------------------------------------------------

/// Outcome of a batch channel write.
#[derive(Serialize)]
pub struct AnytoneWriteChannelsResult {
    /// The 1-based slots that were written.
    pub slots: Vec<usize>,
    /// Bank base addresses actually rewritten (hex), one per affected bank.
    pub banks_written: Vec<String>,
    /// Absolute path of the mandatory pre-write backup of every affected bank.
    pub backup_path: String,
    /// Human note on how to verify (fresh-session read-back after the reboot).
    pub note: String,
}

/// Outcome of a single-field persistent write (the time-slot flip).
#[derive(Serialize)]
pub struct AnytoneWriteFieldResult {
    pub slot: usize,
    pub addr: String,
    /// The record offset written, with a human label (e.g. "0x21 (time slot)").
    pub field: String,
    /// The byte value before the write and the value written.
    pub old_value: u8,
    pub new_value: u8,
    /// Absolute path of the pre-write backup — pass this to `restore_slot_anytone`.
    pub backup_path: String,
    /// The original record decoded, for a human-readable confirmation.
    pub channel: Option<AnytoneDecodedChannel>,
}

/// A plain fresh read of one channel slot, for verification.
#[derive(Serialize)]
pub struct AnytoneSlotReadResult {
    pub slot: usize,
    pub addr: String,
    /// The raw time-slot byte at `CH_TIME_SLOT` (None if the slot is empty).
    pub time_slot_byte: Option<u8>,
    /// The full 128-byte record, hex.
    pub hex: String,
    pub channel: Option<AnytoneDecodedChannel>,
}

/// Outcome of restoring a slot from a backup file.
#[derive(Serialize)]
pub struct AnytoneRestoreResult {
    pub slot: usize,
    pub addr: String,
    /// Absolute path of the backup written back.
    pub backup_path: String,
}

// ------------------------------------------------------------
// Integrity-structure investigation: deterministic dump + diff
//
// A per-record write corrupts the codeplug (the radio maintains a checksum / a
// "valid" marker outside the channel record — see PROJECT_STATE). To find that
// region WITHOUT more risky writes: dump the fixed `DUMP_REGIONS` here, change ONE
// field in RT Systems/CPS, dump again, and DIFF — the region that changed beyond
// the channel record is the integrity structure. The dump is a self-describing
// stream of `[addr:u32 BE][len:u32 BE][data]` blocks so two dumps are byte-aligned
// even if content shifts, and diffs can be reported at real radio addresses.
// ------------------------------------------------------------

#[derive(Serialize)]
pub struct AnytoneDumpResult {
    /// Absolute path of the saved dump file.
    pub path: String,
    /// Total data bytes captured (excludes the per-block headers).
    pub total_bytes: usize,
    /// Per-region capture summary.
    pub regions: Vec<RegionProbe>,
}

