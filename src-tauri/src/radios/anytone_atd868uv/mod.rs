//! AnyTone AT-D868UV driver (`driver_key = "anytone_atd868uv"`, issue #51).
//!
//! The D868 is the base of the family the D578 driver was built on (qdmr's
//! hierarchy is D868UV → D878UV → D578UV), and its RECORDS are the D578's: the
//! channel record's first 0x34 bytes, the zone lists and names, the scan lists,
//! the radio IDs, the contacts and their index list, the present bitmaps and the
//! contact ID table's key format are byte-for-byte the same layout at the same
//! addresses. So this module reuses the D578's record encoders and keeps only
//! what differs:
//!   - the ident (`ID868UVE`, revision `V102` = hardware V1.1 / firmware 2.x);
//!   - the contact ID table and the call-sign database, at their own addresses;
//!   - the channel tail 0x34-0x3F (data ACK, APRS type/index, SMS and key bits
//!     on the D868; the D578's tail is laid out differently), which the encoders
//!     never touch, and the blank-slot template;
//!   - no air band, no channel extension record, DMR APRS only.
//!
//! The general settings block (0xD0 bytes at 0x02500000) has its own layout and
//! is not shared with the D578's.
//!
//! The D890UV is physically the closest radio but shares none of this map.
//!
//! Anchored on a read-only dump of a real D868UV (`ID868UVE\0V102`, 424 channels,
//! 23 zones, 22 scan lists, 40 contacts — `scratchpad/anytone_atd868uv/FINDINGS.md`).
//! The fixtures in `testdata/` are that dump's record regions only; its settings,
//! boot text and radio ID were left out on purpose.

use std::path::Path;

use crate::radios::anytone_atd578uv::program::{preview_model, program_model};
use crate::radios::anytone_atd578uv::{
    check_restore_blocks_for, FamilyModel, CH_REC_LEN, CH_RX_CTCSS, CH_RX_DCS, CH_SCAN_LIST,
    CH_TX_CTCSS, CH_TX_DCS, CTCSS, MAX_CHANNELS, MAX_CONTACTS, MAX_SCAN_LISTS, MAX_ZONES,
    SCAN_MAX_CHANNELS, ZONE_MAX_CHANNELS,
};
use crate::radios::anytone_atd890uv::program::PlanLimits;
use crate::radios::anytone_atd890uv::{
    ascii, end_session, enter_program_and_ident_as, hex, open_port, read_block, IdentExpect, BAUD,
};
use crate::radios::driver::{
    CodeplugPayload, CodeplugPreview, CodeplugProgrammer, DriverDiagnostics, ProgramReport,
    CallsignDbReport, CallsignDbWriter, CallsignRecord, RadioDriver, RadioIdentity, SettingsReader,
    SettingsWriter,
};
use crate::radios::anytone_atd578uv::callsign_db::{write_callsign_db_model, CallsignMap};

pub(crate) mod settings;

pub(crate) struct AnytoneAtd868uv;

pub(crate) static DRIVER: AnytoneAtd868uv = AnytoneAtd868uv;

/// The ident every D868 session requires. The map is firmware 2.x's (qdmr and
/// the dmr-tools fw 2.40 definition), which the `V102` hardware runs; the older
/// V1.0 hardware runs firmware 1.xx, for which there is no published map, so it
/// is refused rather than guessed at.
pub(crate) const D868_IDENT: IdentExpect = IdentExpect {
    token: "868",
    name: "D868UV",
    revision: Some("V102"),
};

/// Contact ID table: the D578's format (u32 LE key = BCD DMR ID << 1 | group
/// flag, then u32 LE contact index, sorted, 0xFF terminator) at the D868's own
/// address. qdmr and dmr-tools both put it here; the anchor radio's 40 contacts
/// rebuild its table byte for byte.
pub(crate) const CONTACT_ID_TABLE: u32 = 0x0434_0000;

/// Call-sign database: the D578's index and record format (the anchor radio's
/// records decode with it, and its limits record holds the real end address,
/// 0x055870D7 for 122,433 entries, as the D578's does), at the D868's own
/// limits and record addresses. 200,000 entries is qdmr's D868 limit, and what
/// the index region holds before the contact ID table at 0x04340000 (thirteen
/// 16,000-entry banks end at 0x0431F400).
pub(crate) const CALLSIGN: CallsignMap = CallsignMap {
    index_base: 0x0400_0000,
    limits_base: 0x044C_0000,
    record_base: 0x0450_0000,
    max_entries: 200_000,
};

/// The record a never-programmed slot starts from: every byte the encoder does
/// not own takes the value the anchor radio wrote on most of its 424 channels.
/// The tail 0x34-0x3F is zero on every one of them. Unlike the D578's template,
/// the CTCSS index bytes are 0x00, the anchor radio's most common value there
/// (190 of 424 channels, a plurality, not a majority; the D578 radio's was
/// 0x0D) — inert either way until a tone-enable bit is set.
pub(crate) fn channel_record_template() -> [u8; CH_REC_LEN] {
    let mut rec = [0u8; CH_REC_LEN];
    rec[CH_TX_CTCSS] = 0x00;
    rec[CH_RX_CTCSS] = 0x00;
    rec[CH_TX_DCS] = 0x11;
    rec[CH_RX_DCS] = 0x11;
    rec[0x10] = 0xCF;
    rec[0x11] = 0x09;
    rec[CH_SCAN_LIST] = 0xFF;
    rec[0x1C] = 0xFF;
    rec
}

/// The planner's limits. Capacities are the D578's records (the same layout
/// and the published D868 limits); the CTCSS table is the family's, with no
/// 150.0 Hz entry. DCS inversion (code + 512) is qdmr's and dmr-tools' and
/// unmeasured here, as on the D578: the anchor radio holds no inverted code.
pub(crate) const D868_LIMITS: PlanLimits = PlanLimits {
    model: "AT-D868UV",
    ctcss: CTCSS,
    dcs_invert: Some(512),
    max_channels: MAX_CHANNELS,
    max_zones: MAX_ZONES,
    zone_max_channels: ZONE_MAX_CHANNELS,
    max_scan_lists: MAX_SCAN_LISTS,
    scan_max_channels: SCAN_MAX_CHANNELS,
    max_contacts: MAX_CONTACTS,
};

/// File-name prefix of a D868 program backup and its expected image, distinct
/// from the D578's and the D890's so no other radio's Verify or Restore accepts
/// one: the D868's map IS the D578's, so an address check alone could not tell
/// their backups apart.
pub(crate) const PROGRAM_PREFIX: &str = "atd868uv-program-";

pub(crate) const D868_MODEL: FamilyModel = FamilyModel {
    ident: D868_IDENT,
    limits: D868_LIMITS,
    contact_id_table: CONTACT_ID_TABLE,
    air_band: false,
    template: channel_record_template,
    program_prefix: PROGRAM_PREFIX,
};

/// Enter PC mode and identify, refusing anything that is not a D868UV `V102`.
pub(crate) fn enter(p: &mut dyn serialport::SerialPort) -> Result<Vec<u8>, String> {
    enter_program_and_ident_as(p, D868_IDENT)
}

/// The restore guard: a backup may hold only the windows a D868 program writes,
/// plus windows carried unchanged beside them in the same flash half.
pub(crate) fn check_restore_blocks(blocks: &[(u32, Vec<u8>)]) -> Result<(), String> {
    check_restore_blocks_for(&D868_MODEL, blocks)
}

impl RadioDriver for AnytoneAtd868uv {
    fn key(&self) -> &'static str {
        "anytone_atd868uv"
    }

    fn display_name(&self) -> &'static str {
        "AnyTone AT-D868UV"
    }

    fn baud(&self) -> u32 {
        BAUD
    }

    fn identify(&self, port: &str) -> Result<RadioIdentity, String> {
        let mut p = open_port(port)?;
        let ident = enter(&mut *p)?;
        // END ends the session; the D868 drops off USB and re-enumerates on its
        // own a few seconds later (measured on the anchor radio).
        let _ = end_session(&mut *p);
        Ok(RadioIdentity {
            matched: ascii(&ident),
            ident_hex: hex(&ident),
            ident_ascii: Some(ascii(&ident)),
        })
    }

    // The programming unit is the whole codeplug (channels + zones + scan lists
    // + contacts in one session), as on the D578 and D890.
    fn as_codeplug_programmer(&self) -> Option<&dyn CodeplugProgrammer> {
        Some(self)
    }

    // Settings are a separate, explicitly chosen write (the dialog's Profile
    // payload); a codeplug program leaves the radio's own alone.
    fn as_settings_reader(&self) -> Option<&dyn SettingsReader> {
        Some(self)
    }

    fn as_settings_writer(&self) -> Option<&dyn SettingsWriter> {
        Some(self)
    }

    fn as_callsign_db_writer(&self) -> Option<&dyn CallsignDbWriter> {
        Some(self)
    }

    fn as_diagnostics(&self) -> Option<&dyn DriverDiagnostics> {
        Some(self)
    }
}

impl CallsignDbWriter for AnytoneAtd868uv {
    /// The whole database DIRECT, as on the D578: every index and record bank
    /// sits alone at the start of its 128 KB flash half, so each is written
    /// whole with no read phase. One session, END once.
    fn write_callsign_db(&self, port: &str, records: &[CallsignRecord]) -> Result<CallsignDbReport, String> {
        write_callsign_db_model(&D868_MODEL, &CALLSIGN, port, records)
    }
}

impl CodeplugProgrammer for AnytoneAtd868uv {
    fn preview(&self, payload: &CodeplugPayload) -> Result<CodeplugPreview, String> {
        preview_model(&D868_MODEL, payload)
    }

    fn program(
        &self,
        port: &str,
        payload: &CodeplugPayload,
        backup_dir: &Path,
    ) -> Result<ProgramReport, String> {
        program_model(&D868_MODEL, port, payload, backup_dir)
    }
}

impl DriverDiagnostics for AnytoneAtd868uv {
    fn dump_raw(&self, port: &str, start: u32, len: u32) -> Result<Vec<u8>, String> {
        let mut p = open_port(port)?;
        let _ident = enter(&mut *p)?;
        let data = read_block(&mut *p, start, len as usize);
        let _ = end_session(&mut *p);
        data
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod hw_ladder;
