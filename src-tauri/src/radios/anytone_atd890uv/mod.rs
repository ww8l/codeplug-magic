//! AnyTone AT-D890UV driver (`driver_key = "anytone_atd890uv"`).
//!
//! The largest driver in the app: a faithful Rust implementation of the AnyTone
//! D868/D878/D578/D890 clone protocol so Codeplug Magic can read from and program
//! the radio over its USB-C (CDC-ACM) port without AnyTone's CPS. Moved here from
//! `commands/anytone*.rs` in Chunk 3.5 (mechanical extraction); the Tauri command
//! layer under `commands/` still owns DB access, backup paths, and result DTOs,
//! and re-exports these items so its command bodies (and the RE example binaries)
//! keep compiling.
//!
//! This module (`mod.rs`) is the core protocol + channel encode/decode + the
//! read-modify-write bank/window write engine. The capability halves live in
//! submodules: `program` (ChannelWriter + DB→plan assembly), `settings`
//! (SettingsProgrammer), `callsign_db` (CallsignDbWriter), `export`
//! (CodeplugExporter). See `codeplug-magic-launch-plan.md` Chunk 3.
//!
//! Protocol notes (documented by the reald/anytone-flash-tools project and the
//! qdmr `anytone_interface` transport; the wire format is shared across the
//! D868/D878/D578/D890 family):
//!   - serial: 8-N-1; baud is driver-set and effectively ignored (CDC-ACM).
//!   - enter program mode: send ASCII `PROGRAM`; radio replies `QX\x06`.
//!   - identify: send `0x02`; radio replies a ~16-byte ident terminated by `0x06`,
//!     ASCII like `ID890UV...`; the `IDxxxxxx` token must contain "890".
//!   - read block: `R`(0x52) + addr(4 BE) + size(1); reply `W`(0x57) + addr(4) +
//!     size(1) + data + checksum(1, = addr+len+data) + ack(0x06).
//!   - end session: send ASCII `END`; radio replies `0x06`.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serialport::{ClearBuffer, SerialPort};

use crate::error::MapErrString;
use crate::radios::driver::{
    CallsignDbWriter, CodeplugExporter, CodeplugProgrammer, DriverDiagnostics, RadioDriver,
    RadioIdentity, SettingsProgrammer,
};

pub(crate) mod callsign_db;
pub(crate) mod export;
pub(crate) mod program;
pub(crate) mod settings;

// ============================================================
// Driver registration (RadioDriver + capability sub-traits)
// ============================================================

/// The AnyTone AT-D890UV driver unit type. All protocol state lives on the serial
/// port; the driver itself is stateless, so a single static instance serves the
/// registry. Capability sub-traits are implemented in the submodules (added as
/// each is extracted in Chunk 3.5).
pub(crate) struct AnytoneAtd890uv;

/// Registry entry (see `radios/registry.rs`). `identify` reaches it through the
/// registry as of 3.6c; the remaining commands still call the module functions
/// directly (re-exported through `commands::anytone`) until 3.6d/3.6e.
pub(crate) static DRIVER: AnytoneAtd890uv = AnytoneAtd890uv;

impl RadioDriver for AnytoneAtd890uv {
    fn key(&self) -> &'static str {
        "anytone_atd890uv"
    }

    fn display_name(&self) -> &'static str {
        "AnyTone AT-D890UV"
    }

    fn baud(&self) -> u32 {
        BAUD
    }

    fn identify(&self, port: &str) -> Result<RadioIdentity, String> {
        let mut p = open_port(port)?;
        let ident = enter_program_and_ident(&mut *p)?;
        // END reboots the radio and re-enumerates the USB port, so it must be
        // the last thing this session does — see `anytone-direct-cable-programming`.
        let _ = end_session(&mut *p);
        Ok(RadioIdentity {
            matched: ascii(&ident),
            ident_hex: hex(&ident),
            ident_ascii: Some(ascii(&ident)),
        })
    }

    // Capabilities the driver advertises. `as_channel_writer` stays unset on
    // purpose: this radio's programming unit is the whole codeplug (channels +
    // zones + scan lists + contacts in one session), which is
    // `CodeplugProgrammer`, not the channels-only `ChannelWriter`.
    fn as_codeplug_programmer(&self) -> Option<&dyn CodeplugProgrammer> {
        Some(self)
    }

    fn as_settings_programmer(&self) -> Option<&dyn SettingsProgrammer> {
        Some(self)
    }

    fn as_callsign_db_writer(&self) -> Option<&dyn CallsignDbWriter> {
        Some(self)
    }

    fn as_codeplug_exporter(&self) -> Option<&dyn CodeplugExporter> {
        Some(self)
    }

    fn as_diagnostics(&self) -> Option<&dyn DriverDiagnostics> {
        Some(self)
    }
}

impl DriverDiagnostics for AnytoneAtd890uv {
    /// Read `len` bytes at absolute address `start` in one PC-mode session
    /// (enter → strict-checksum read → END). Backs the raw dump/diff RE tooling.
    fn dump_raw(&self, port: &str, start: u32, len: u32) -> Result<Vec<u8>, String> {
        let mut p = open_port(port)?;
        let _ident = enter_program_and_ident(&mut *p)?;
        let data = read_block(&mut *p, start, len as usize);
        let _ = end_session(&mut *p);
        data
    }
}

/// CDC-ACM: the cable/driver sets the real line rate, so this value is nominal.
pub(crate) const BAUD: u32 = 115_200;
pub(crate) const TIMEOUT: Duration = Duration::from_secs(1);

/// Enter-program-mode command and its expected reply ("QX" + ack).
pub(crate) const PROGRAM: &[u8] = b"PROGRAM";
pub(crate) const PROGRAM_ACK: &[u8] = &[0x51, 0x58, 0x06];
/// Clean session terminator; the radio replies with a single ack.
pub(crate) const END: &[u8] = b"END";
pub(crate) const ACK: u8 = 0x06;

/// Bytes per read frame. 0x10 is the documented typical chunk.
pub(crate) const READ_FRAME: u8 = 0x10;

/// D890UV codeplug regions to probe, from the dmr-tools/codeplugs `d890uv`
/// definition (v1.05, machine-generated by anytone-emu) — the radio's REAL map,
/// which differs from the D868/D878 family. Kept small so a single run captures
/// real bytes from each region. `(name, address, len)`.
pub(crate) const PROBE_REGIONS: &[(&str, u32, u32)] = &[
    ("general_settings", 0x0350_0000, 0x0100),
    // Channel banks (read_channel_banks) and zones (read_zones) are read
    // separately so Stage 2 decode covers every programmed slot/zone.
    ("scan_list_1", 0x0210_0000, 0x0040),
    ("fm_channels", 0x0340_0000, 0x0040),
];

/// Channel storage: 32 banks of 128 records each (4000 channels, last bank
/// partial). Bank N base = `CHANNEL_BASE + N*BANK_STEP`; each bank is
/// `CH_PER_BANK * CH_REC_LEN` = 0x4000 bytes. Confirmed against a programmed
/// D890UV via the dmr-tools `d890uv` map.
pub(crate) const CHANNEL_BASE: u32 = 0x0100_0000;
pub(crate) const BANK_STEP: u32 = 0x0008_0000;
pub(crate) const NUM_BANKS: usize = 32;

/// Zones, from the dmr-tools `d890uv` map. Channel lists are 250 zones of 250
/// 0-based channel indices (u16 LE, 0xFFFF = unset) at `ZONE_LIST_BASE` step
/// 0x200. Names are a parallel array of 16-char UTF-16 strings at
/// `ZONE_NAME_BASE` step 0x40. The radio shows a zone iff its bit is set in the
/// zone-present bitmap at `ZONE_BITMAP_BASE` (1 bit/zone, LSB = zone 1,
/// `ZONE_BITMAP_BYTES` bytes) — NOT by name-presence. HW-verified 2026-07-09:
/// adding a zone on the radio flips exactly one bit here (0x01→0x03 for zone 2);
/// leaving stale bits set is what produced phantom (nameless "BÖLGE N") zones
/// after a codeplug with fewer zones was written. NOTE: the dmr-tools map's
/// "0x03482C20" is off by 0x20 — that is the END of this 32-byte bitmap.
pub(crate) const ZONE_LIST_BASE: u32 = 0x0200_0000;
pub(crate) const ZONE_LIST_STEP: u32 = 0x0000_0200;
pub(crate) const ZONE_NAME_BASE: u32 = 0x0360_0000;
pub(crate) const ZONE_NAME_STEP: u32 = 0x0000_0040;
pub(crate) const ZONE_NAME_CHARS: usize = 16;
pub(crate) const MAX_ZONES: usize = 250;
/// Zone-present bitmap: 1 bit per zone (LSB = zone 1), `ceil(250/8)` = 32 bytes.
pub(crate) const ZONE_BITMAP_BASE: u32 = 0x0348_2C00;
pub(crate) const ZONE_BITMAP_BYTES: usize = 32;

/// Scan lists. Layout HW-pinned (s53) by differential dumps against AnyTone's
/// own CPS writes on a live D890UV: the member ARRAY is 100 slots (0x30–0xF7,
/// CPS 0xFF-fills the unused tail of it) and the revert enum is @0xF8 — the
/// original descriptor/qdmr layout was right. s51's 50-member/0x94 conclusion
/// came from RT Systems, whose OWN layout is buggy: it writes its revert byte
/// at 0x94 (member slot #50), which the radio ignores.
/// 250 records of 0x200 bytes at `SCAN_LIST_BASE`: unused(1) + priority
/// enum(1) (HW: 0=Off, 1=Ch1) + primary/secondary priority channel index
/// (u16 LE, idx+1, 0xFFFF=none; offsets + idx+1 encoding HW-confirmed) + four
/// u16 LE timers (0.1 s units) + 16-char UTF-16 name @0x0E + 2 unused +
/// 100 × u16 LE 0-based channel indices @0x30 (0xFFFF = unset) + revert enum
/// @0xF8 (HW-confirmed: 4=Last Called, 5=Last Used) + zero-filled tail.
pub(crate) const SCAN_LIST_BASE: u32 = 0x0210_0000;
pub(crate) const SCAN_LIST_STEP: u32 = 0x0000_0200;
pub(crate) const MAX_SCAN_LISTS: usize = 250;
/// Functional member cap: AnyTone documents 50 channels per scan list, so we
/// validate/truncate at 50 even though the record's array has 100 slots.
pub(crate) const SCAN_MAX_CHANNELS: usize = 50;
pub(crate) const SCAN_NAME: usize = 0x0E;
pub(crate) const SCAN_NAME_CHARS: usize = 16;
pub(crate) const SCAN_MEMBERS: usize = 0x30;
/// Size of the member array in the record (u16 LE slots), > the 50-member cap.
pub(crate) const SCAN_MEMBER_SLOTS: usize = 100;
/// Revert Channel enum byte — immediately after the 100-slot member array.
pub(crate) const SCAN_REVERT: usize = SCAN_MEMBERS + SCAN_MEMBER_SLOTS * 2; // 0xF8

/// DMR Contact bank, from the dmr-tools `d890uv` map. Records are 0xC8 bytes at
/// `CONTACT_BASE`, indexed directly by a channel's contact index (record 0x14).
/// Layout: Call Type (1B), Call Alert (1B), DMR ID (4B BCD big-endian), Name
/// (16-char UTF-16), then 0xA2 unused. We read only up to the highest index any
/// channel actually references. Capped so a stray high index can't trigger a
/// multi-MB read.
pub(crate) const CONTACT_BASE: u32 = 0x03A0_0000;
pub(crate) const CONTACT_REC_LEN: usize = 0xC8;
pub(crate) const CONTACT_CALL_TYPE: usize = 0x00;
pub(crate) const CONTACT_DMR_ID: usize = 0x02;
pub(crate) const CONTACT_NAME: usize = 0x06;
pub(crate) const CONTACT_NAME_CHARS: usize = 16;
pub(crate) const MAX_CONTACTS_READ: usize = 4000;

// ============================================================
// Tauri commands
// ============================================================

/// One probed region's outcome.
#[derive(Serialize)]
pub struct RegionProbe {
    pub name: String,
    /// Region base address, hex (e.g. "0x02500000").
    pub addr: String,
    /// Bytes requested vs actually read before a stop/error.
    pub requested: usize,
    pub read: usize,
    /// Whether every frame's checksum/ack matched our assumed algorithm. If data
    /// came back but this is false, the framing is right but the checksum
    /// formula differs — still useful, just flagged.
    pub checksum_ok: bool,
    /// Hex preview of the first bytes read (for eyeballing / pasting back).
    pub preview: String,
    /// Which checksum formula matched the radio's first-frame checksum byte
    /// (derived live), e.g. "addr+len+data (raw=0x42)". Lets us bake in the
    /// right formula. None if the region didn't respond.
    pub checksum_formula: Option<String>,
    /// The error that stopped this region, if any (e.g. a timeout = no such
    /// address on this model).
    pub error: Option<String>,
}

/// A decoded channel record. Every field below is hardware-confirmed against a
/// programmed D890UV (see the channel-record map in PROJECT_STATE / memory).
/// Deserialize so the frontend can hand a downloaded set back to
/// `import_anytone_download` for the library import.
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct AnytoneDecodedChannel {
    /// 1-based memory slot number (matches CPS channel numbering).
    pub index: usize,
    pub name: String,
    pub rx_mhz: f64,
    /// Actual TX frequency in MHz (rx ± offset per the shift-direction bits;
    /// equals `rx_mhz` for simplex). The structured form of `shift`.
    pub tx_mhz: f64,
    /// Signed TX shift as the radio applies it: "" simplex, "+5.000", "-5.000"
    /// (derived from the offset magnitude at 0x04 + the shift-direction bits in
    /// the mode byte at 0x08).
    pub shift: String,
    /// Channel mode: "FM", "DMR", "FM+DMR RX", or "DMR+FM RX" (mode-byte bits 1-0).
    pub mode: String,
    /// TX power: "Low", "Medium", "High", or "Turbo" (mode-byte bits 3-2).
    pub power: String,
    /// Channel bandwidth: "12.5 kHz" or "25 kHz" (mode-byte bits 5-4).
    pub bandwidth: String,
    /// Analog CTCSS/DCS summary, e.g. "CTCSS 100.0", "TX CTCSS 100.0 / RX DCS 023",
    /// or "—". "—" for pure-DMR channels.
    pub tone: String,
    /// Structured TX / RX sub-tones behind the `tone` label (None on either side
    /// = no tone; both None for digital channels). CTCSS carries Hz, DCS the raw
    /// stored code — same convention as the Stage 3 edit model.
    pub tone_tx: Option<AnytoneSubTone>,
    pub tone_rx: Option<AnytoneSubTone>,
    /// DMR color code 0-15, or null for analog.
    pub color_code: Option<u8>,
    /// DMR time slot (1 or 2), or null for analog.
    pub time_slot: Option<u8>,
    /// DMR contact (talkgroup) index into the radio's contact list, or null.
    pub contact_index: Option<u16>,
    /// DMR contact (talkgroup) name resolved from the Contact bank, or null if
    /// analog or the contact slot wasn't read.
    pub contact_name: Option<String>,
}

/// A decoded zone: its ordered member channels resolved to names where the
/// referenced slot was among the banks we read (else shown as "#<n>").
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct AnytoneDecodedZone {
    /// 1-based zone number.
    pub index: usize,
    /// Zone name, decoded from the Zone Names region.
    pub name: String,
    pub channels: Vec<String>,
    /// The raw ordered member list: 0-based channel slot indices as stored in
    /// the zone record. The structured form of `channels`, for the DB import.
    pub member_slots: Vec<u16>,
}

/// A decoded DMR contact (talkgroup) record. Only contacts some decoded channel
/// references are returned. Field offsets are HW-confirmed (see
/// `contact_write_patches`).
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct AnytoneDecodedContact {
    /// 0-based contact index (what channel records reference at 0x14).
    pub index: u16,
    pub name: String,
    /// The DMR ID / talkgroup number (4-byte BE BCD at 0x02).
    pub dmr_id: u32,
    /// Call type: 0 = Private, 1 = Group, 2 = All.
    pub call_type: u8,
}

/// One differing byte-run between two dumps, at its real radio address.
#[derive(Serialize, PartialEq, Debug)]
pub struct AnytoneDumpDiff {
    pub addr: String,
    pub len: usize,
    pub a_hex: String,
    pub b_hex: String,
}

/// Parse a self-describing dump into `(addr, data)` blocks.
pub(crate) fn parse_dump(bytes: &[u8]) -> Result<Vec<(u32, Vec<u8>)>, String> {
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if i + 8 > bytes.len() {
            return Err("truncated block header".into());
        }
        let addr = u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        let len =
            u32::from_be_bytes([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]]) as usize;
        i += 8;
        if i + len > bytes.len() {
            return Err(format!("truncated block data at 0x{addr:08X}"));
        }
        blocks.push((addr, bytes[i..i + len].to_vec()));
        i += len;
    }
    Ok(blocks)
}

/// Compare two parsed dumps block-by-block (same fixed `DUMP_REGIONS` order) and
/// collect differing byte-runs with their absolute radio address.
pub(crate) fn diff_dumps(a: &[(u32, Vec<u8>)], b: &[(u32, Vec<u8>)]) -> Vec<AnytoneDumpDiff> {
    let mut diffs = Vec::new();
    for (idx, (addr_a, data_a)) in a.iter().enumerate() {
        let Some((addr_b, data_b)) = b.get(idx) else {
            break;
        };
        if addr_a != addr_b {
            // Region maps differ — shouldn't happen with a fixed DUMP_REGIONS.
            diffs.push(AnytoneDumpDiff {
                addr: format!("0x{addr_a:08X}"),
                len: 0,
                a_hex: format!("block@0x{addr_a:08X}"),
                b_hex: format!("block@0x{addr_b:08X}"),
            });
            continue;
        }
        let n = data_a.len().min(data_b.len());
        let mut j = 0;
        while j < n {
            if data_a[j] == data_b[j] {
                j += 1;
                continue;
            }
            let start = j;
            while j < n && data_a[j] != data_b[j] {
                j += 1;
            }
            diffs.push(AnytoneDumpDiff {
                addr: format!("0x{:08X}", addr_a + start as u32),
                len: j - start,
                a_hex: hex(&data_a[start..j]),
                b_hex: hex(&data_b[start..j]),
            });
        }
        if data_a.len() != data_b.len() {
            let from = n;
            let longer = if data_a.len() > data_b.len() { data_a } else { data_b };
            diffs.push(AnytoneDumpDiff {
                addr: format!("0x{:08X}", addr_a + from as u32),
                len: longer.len() - n,
                a_hex: if data_a.len() > n { hex(&data_a[n..]) } else { String::new() },
                b_hex: if data_b.len() > n { hex(&data_b[n..]) } else { String::new() },
            });
        }
    }
    diffs
}

// ============================================================
// Serial protocol
// ============================================================

pub(crate) fn open_port(port: &str) -> Result<Box<dyn SerialPort>, String> {
    serialport::new(port, BAUD)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        .timeout(TIMEOUT)
        .open()
        .map_err(|e| {
            let base = format!("could not open {port}: {e}");
            // A vanished device node means the radio re-enumerated — AnyTone
            // radios drop off USB (and often come back under a NEW name) when
            // they leave PC mode after the previous operation.
            let missing = matches!(e.kind(), serialport::ErrorKind::NoDevice)
                || e.to_string().contains("No such file");
            if missing {
                format!(
                    "{base}\n\nThe port is no longer present — the radio likely \
                     re-enumerated (AnyTone radios drop off USB when they leave PC \
                     mode). Power-cycle/re-seat the radio, wait for it to finish \
                     booting, click Rescan, and reselect the port (its name may have \
                     changed)."
                )
            } else {
                base
            }
        })
}

/// Read a single byte, or `None` on timeout (no data).
pub(crate) fn read_byte(p: &mut dyn SerialPort) -> Result<Option<u8>, String> {
    let mut b = [0u8; 1];
    match std::io::Read::read(p, &mut b) {
        Ok(0) => Ok(None),
        Ok(_) => Ok(Some(b[0])),
        Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Read exactly `n` bytes, erroring if the radio goes quiet mid-frame.
pub(crate) fn read_exact(p: &mut dyn SerialPort, n: usize) -> Result<Vec<u8>, String> {
    let mut buf = Vec::with_capacity(n);
    let mut empty = 0;
    while buf.len() < n {
        match read_byte(p)? {
            Some(b) => {
                buf.push(b);
                empty = 0;
            }
            None => {
                empty += 1;
                if empty > 3 {
                    return Err(format!("timed out: expected {n} bytes, got {}", buf.len()));
                }
            }
        }
    }
    Ok(buf)
}

/// Enter PC/clone mode and read the ident, retrying the handshake. AnyTone
/// radios frequently ignore the first `PROGRAM` after a previous session (the
/// last op left them mid-reboot or briefly busy), so a single timeout is not a
/// real failure — clear the buffer and try again over a few-second window, the
/// same approach the UV-5R driver uses (`reident_with_retry`).
pub(crate) fn enter_program_and_ident(p: &mut dyn SerialPort) -> Result<Vec<u8>, String> {
    const ATTEMPTS: usize = 4;
    let mut last = String::from("no response");
    for attempt in 0..ATTEMPTS {
        let _ = p.clear(ClearBuffer::All);
        // Let the port settle after the clear before talking.
        std::thread::sleep(Duration::from_millis(200));
        match try_enter_program_and_ident(p) {
            Ok(ident) => return Ok(ident),
            Err(e) => {
                last = e;
                if attempt + 1 < ATTEMPTS {
                    std::thread::sleep(Duration::from_millis(700));
                }
            }
        }
    }
    Err(format!(
        "could not enter PC mode after {ATTEMPTS} tries ({last}). \
         Make sure the D890UV is powered on and idle — if its screen already \
         shows \"PC Mode\", power-cycle the radio and try again."
    ))
}

/// One handshake attempt: `PROGRAM`, verify the `QX\x06` reply, send `0x02`, and
/// read the ident terminated by `0x06`.
pub(crate) fn try_enter_program_and_ident(p: &mut dyn SerialPort) -> Result<Vec<u8>, String> {
    p.write_all(PROGRAM).estr()?;
    let ack = read_exact(p, PROGRAM_ACK.len())?;
    if ack != PROGRAM_ACK {
        return Err(format!(
            "radio did not ack PROGRAM (got {}, expected QX+ack)",
            hex(&ack)
        ));
    }

    p.write_all(&[0x02]).estr()?;
    // Ident is a short ASCII record terminated by 0x06; read up to 16 bytes.
    let mut ident = Vec::new();
    for _ in 0..16 {
        match read_byte(p)? {
            Some(b) if b == ACK => break,
            Some(b) => ident.push(b),
            None => break,
        }
    }
    if ident.is_empty() {
        return Err("no ident returned after entering PC mode".into());
    }
    if !ident_is_d890(&ident) {
        return Err(format!(
            "connected radio identified as '{}', not a D890UV",
            ascii(&ident).trim_end_matches('.')
        ));
    }
    Ok(ident)
}

/// Does the ident name a D890UV? The token is ASCII like `ID890UV...`.
pub(crate) fn ident_is_d890(ident: &[u8]) -> bool {
    ascii(ident).contains("890")
}

/// Probe one region: read it in `READ_FRAME`-sized frames until done or the
/// radio stops responding. Never errors — captures partial results and the stop
/// reason so a wrong (unmapped) address just reports a timeout instead of
/// aborting the whole run.
pub(crate) fn probe_region(
    p: &mut dyn SerialPort,
    name: &str,
    addr: u32,
    len: u32,
) -> (RegionProbe, Vec<u8>) {
    let mut data = Vec::new();
    let mut checksum_ok = true;
    let mut checksum_formula = None;
    let mut error = None;
    let mut a = addr;
    let end = addr + len;
    while a < end {
        match read_frame_lenient(p, a, READ_FRAME) {
            Ok((chunk, raw)) => {
                checksum_ok &= raw == anytone_checksum(&a.to_be_bytes(), &chunk);
                // Derive the formula from the first responding frame.
                if checksum_formula.is_none() {
                    checksum_formula = Some(identify_checksum(a, READ_FRAME, &chunk, raw));
                }
                data.extend(chunk);
                a += READ_FRAME as u32;
            }
            Err(e) => {
                error = Some(e);
                break;
            }
        }
    }
    let probe = RegionProbe {
        name: name.to_string(),
        addr: format!("0x{addr:08X}"),
        requested: len as usize,
        read: data.len(),
        checksum_ok: !data.is_empty() && checksum_ok,
        preview: hex(&data[..data.len().min(32)]),
        checksum_formula,
        error,
    };
    (probe, data)
}

/// Read all channel banks for Stage 2 decode. Returns each bank's probe report
/// plus `(slot_base, bytes)` for every bank that returned data.
///
/// AnyTone allocates channels into banks sequentially, so once a bank comes back
/// fully empty (all 0xFF) the remaining banks are empty too — stop there rather
/// than reading the full 512 KB every time. A bank holds 128 slots, so only a
/// run of 128 consecutive empty slots ends the scan, which in practice only
/// happens past the last programmed channel.
pub(crate) fn read_channel_banks(p: &mut dyn SerialPort) -> (Vec<RegionProbe>, Vec<(usize, Vec<u8>)>) {
    let bank_len = (CH_PER_BANK * CH_REC_LEN) as u32;
    let mut regions = Vec::new();
    let mut banks = Vec::new();
    for i in 0..NUM_BANKS {
        let name = format!("channel_bank_{i}");
        let addr = CHANNEL_BASE + (i as u32) * BANK_STEP;
        let (probe, data) = probe_region(p, &name, addr, bank_len);
        let empty = data.is_empty() || data.iter().all(|&b| b == 0xFF);
        regions.push(probe);
        if !empty {
            banks.push((i * CH_PER_BANK, data));
        } else if !data.is_empty() {
            // A fully-empty bank that still responded marks the end of allocation.
            break;
        }
    }
    (regions, banks)
}

/// Read every populated zone: its name (from the Zone Names region) and its
/// channel list (from the Zone channel-lists region), resolving member slots to
/// channel names via `slot_names`. Returns the per-region probe reports, the
/// captured bytes (for the backup image), and the decoded zones.
///
/// Zones are allocated sequentially with no "present" bitmap, so a zone with an
/// empty name marks the end — stop there rather than reading all 250 zones.
pub(crate) fn read_zones(
    p: &mut dyn SerialPort,
    slot_names: &HashMap<usize, String>,
) -> (Vec<RegionProbe>, Vec<u8>, Vec<AnytoneDecodedZone>) {
    let mut regions = Vec::new();
    let mut image = Vec::new();
    let mut zones = Vec::new();
    for i in 0..MAX_ZONES {
        let name_addr = ZONE_NAME_BASE + (i as u32) * ZONE_NAME_STEP;
        let (name_probe, name_data) =
            probe_region(p, &format!("zone_{}_name", i + 1), name_addr, ZONE_NAME_STEP);
        image.extend_from_slice(&name_data);
        regions.push(name_probe);

        let name = decode_utf16_name(&name_data, ZONE_NAME_CHARS);
        if name.is_empty() {
            // A responding-but-empty name ends the sequential zone allocation; an
            // empty read (timeout) is just skipped so a transient error isn't fatal.
            if name_data.is_empty() {
                continue;
            }
            break;
        }

        let list_addr = ZONE_LIST_BASE + (i as u32) * ZONE_LIST_STEP;
        let (list_probe, list_data) =
            probe_region(p, &format!("zone_{}_list", i + 1), list_addr, ZONE_LIST_STEP);
        image.extend_from_slice(&list_data);
        regions.push(list_probe);

        zones.push(decode_zone(&list_data, slot_names, name, i + 1));
    }
    (regions, image, zones)
}

/// Read the DMR Contact bank far enough to resolve every talkgroup any decoded
/// channel references, and return an `index → contact` map. Reads one contiguous
/// block from `CONTACT_BASE` up to the highest referenced index, so an all-analog
/// codeplug reads nothing. Returns the probe report, the captured bytes, and the
/// resolution map.
pub(crate) fn read_contacts(
    p: &mut dyn SerialPort,
    channels: &[AnytoneDecodedChannel],
) -> (Vec<RegionProbe>, Vec<u8>, HashMap<u16, AnytoneDecodedContact>) {
    let max_idx = channels
        .iter()
        .filter_map(|c| c.contact_index)
        .max()
        .map(|m| m as usize);
    let Some(max_idx) = max_idx else {
        return (Vec::new(), Vec::new(), HashMap::new());
    };
    let count = (max_idx + 1).min(MAX_CONTACTS_READ);
    let len = (count * CONTACT_REC_LEN) as u32;
    let (probe, data) = probe_region(p, "contacts", CONTACT_BASE, len);
    let contacts = decode_contacts(&data, channels);
    (vec![probe], data, contacts)
}

/// Build the `contact index → contact` map from a Contact-bank block, for the
/// indices that channels actually reference. Each record is `CONTACT_REC_LEN`
/// bytes: call type at 0x00, DMR ID (BE BCD) at 0x02, UTF-16 name at
/// `CONTACT_NAME`; unprogrammed/blank records are skipped so an unresolved index
/// falls back to its number in the UI.
pub(crate) fn decode_contacts(
    data: &[u8],
    channels: &[AnytoneDecodedChannel],
) -> HashMap<u16, AnytoneDecodedContact> {
    let mut contacts = HashMap::new();
    let mut wanted: Vec<u16> = channels.iter().filter_map(|c| c.contact_index).collect();
    wanted.sort_unstable();
    wanted.dedup();
    for idx in wanted {
        let start = idx as usize * CONTACT_REC_LEN;
        let Some(rec) = data.get(start..start + CONTACT_REC_LEN) else {
            continue;
        };
        if is_empty_record(rec) {
            continue;
        }
        let name = decode_utf16_name(&rec[CONTACT_NAME..], CONTACT_NAME_CHARS);
        if !name.is_empty() {
            contacts.insert(
                idx,
                AnytoneDecodedContact {
                    index: idx,
                    name,
                    dmr_id: bcd_be(&rec[CONTACT_DMR_ID..CONTACT_DMR_ID + 4]) as u32,
                    call_type: rec[CONTACT_CALL_TYPE],
                },
            );
        }
    }
    contacts
}

/// Read one frame leniently: validate the response header (so framing desync is
/// caught and reported), but return the data even when the checksum/ack don't
/// match — during reverse engineering a checksum-formula mismatch shouldn't
/// discard real bytes. Returns `(data, raw_checksum_byte)`.
pub(crate) fn read_frame_lenient(
    p: &mut dyn SerialPort,
    addr: u32,
    size: u8,
) -> Result<(Vec<u8>, u8), String> {
    let a = addr.to_be_bytes();
    let msg = [b'R', a[0], a[1], a[2], a[3], size];
    p.write_all(&msg).estr()?;

    // Response: W(1) + addr(4) + len(1) + data(size) + checksum(1) + ack(1).
    let header = read_exact(p, 6)?;
    validate_read_header(addr, size, &header)?;
    let data = read_exact(p, size as usize)?;
    let tail = read_exact(p, 2)?;
    Ok((data, tail[0]))
}

/// Identify which checksum formula the radio uses by testing candidate sums
/// against the actual checksum byte. Returns a short label so the right formula
/// can be baked in once it's known. `header` is the 6-byte response header
/// (`W` + addr + len) that some firmwares fold into the checksum.
pub(crate) fn identify_checksum(addr: u32, size: u8, data: &[u8], raw: u8) -> String {
    let a = addr.to_be_bytes();
    let sum = |bytes: &[u8]| bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
    let sum_data = sum(data);
    let sum_addr = sum(&a);
    let candidates: [(&str, u8); 5] = [
        ("data", sum_data),
        ("addr+data", sum_addr.wrapping_add(sum_data)),
        ("addr+len+data", sum_addr.wrapping_add(size).wrapping_add(sum_data)),
        (
            "cmd+addr+len+data",
            0x57u8
                .wrapping_add(sum_addr)
                .wrapping_add(size)
                .wrapping_add(sum_data),
        ),
        ("len+data", size.wrapping_add(sum_data)),
    ];
    for (name, val) in candidates {
        if val == raw {
            return format!("{name} (raw=0x{raw:02X})");
        }
    }
    format!("unknown (raw=0x{raw:02X}, no candidate matched)")
}

/// Validate a read-response header echoes our command. Pure (no I/O) so it is
/// unit-testable.
pub(crate) fn validate_read_header(addr: u32, size: u8, header: &[u8]) -> Result<(), String> {
    let a = addr.to_be_bytes();
    if header.len() != 6 || header[0] != b'W' || header[1..5] != a || header[5] != size {
        return Err(format!("bad header for frame 0x{addr:08X}: {}", hex(header)));
    }
    Ok(())
}

/// 1-byte additive checksum over the 4 address bytes, the length byte, and the
/// data. HARDWARE-CONFIRMED as the D890UV's per-frame formula: `identify_checksum`
/// matched `addr+len+data` on every region of a full radio read (session 18). The
/// length byte is the frame's data length (always == `data.len()`), so it's
/// derived here rather than passed. Used for both read verification and the write
/// frame (clone protocols share the algorithm both directions).
pub(crate) fn anytone_checksum(addr: &[u8; 4], data: &[u8]) -> u8 {
    let len = data.len() as u8;
    addr.iter()
        .chain(std::iter::once(&len))
        .chain(data.iter())
        .fold(0u8, |acc, &b| acc.wrapping_add(b))
}

/// Build one write-command frame: `W`(0x57) + addr(BE32) + len(1) + data +
/// checksum(1) + a trailing `ACK`(0x06) TERMINATOR. The trailing 0x06 is part of
/// the bytes WE send (mirroring the read *response*, which also ends in 0x06) —
/// without it the radio never treats the frame as complete and stays silent. The
/// radio then replies with its own single `ACK` byte. Pure (no I/O) so the framing
/// is unit-testable. `data` must be at most 255 bytes (one length byte).
pub(crate) fn build_write_frame(addr: u32, data: &[u8]) -> Vec<u8> {
    debug_assert!(data.len() <= u8::MAX as usize, "write frame data too long");
    let a = addr.to_be_bytes();
    let mut frame = Vec::with_capacity(6 + data.len() + 2);
    frame.push(b'W');
    frame.extend_from_slice(&a);
    frame.push(data.len() as u8);
    frame.extend_from_slice(data);
    frame.push(anytone_checksum(&a, data));
    frame.push(ACK);
    frame
}

/// Byte address of a 1-based channel slot's 128-byte record. Slots are laid out
/// `CH_PER_BANK` per bank across the 32 channel banks (see `read_channel_banks`).
pub(crate) fn slot_addr(slot: usize) -> u32 {
    let zero = slot - 1;
    let bank = zero / CH_PER_BANK;
    let within = zero % CH_PER_BANK;
    CHANNEL_BASE + (bank as u32) * BANK_STEP + (within as u32) * CH_REC_LEN as u32
}

/// Read `len` bytes from `addr` STRICTLY (every frame's checksum must match, so a
/// garbled read can't be mistaken for real content), in `READ_FRAME`-sized frames.
pub(crate) fn read_block(p: &mut dyn SerialPort, addr: u32, len: usize) -> Result<Vec<u8>, String> {
    let mut data = Vec::with_capacity(len);
    let mut a = addr;
    while data.len() < len {
        let (chunk, raw) = read_frame_lenient(p, a, READ_FRAME)?;
        if raw != anytone_checksum(&a.to_be_bytes(), &chunk) {
            return Err(format!(
                "read checksum mismatch at 0x{a:08X} (raw=0x{raw:02X})"
            ));
        }
        data.extend(chunk);
        a += READ_FRAME as u32;
    }
    data.truncate(len);
    Ok(data)
}

/// Read one 128-byte channel record strictly.
pub(crate) fn read_record(p: &mut dyn SerialPort, addr: u32) -> Result<Vec<u8>, String> {
    read_block(p, addr, CH_REC_LEN)
}

/// Write `data` to `addr` as a CONTIGUOUS sweep of `READ_FRAME`-sized frames —
/// mirroring the exact chunking/addresses the radio streamed on read, the way CPS
/// and qdmr push an image. Reads the single-byte `ACK` after each frame.
/// `data.len()` should be a multiple of the frame size.
pub(crate) fn write_block(p: &mut dyn SerialPort, addr: u32, data: &[u8]) -> Result<(), String> {
    let step = READ_FRAME as usize;
    for (i, chunk) in data.chunks(step).enumerate() {
        let a = addr + (i * step) as u32;
        let frame = build_write_frame(a, chunk);
        p.write_all(&frame).estr()?;
        match read_byte(p)? {
            Some(ACK) => {}
            other => {
                return Err(format!(
                    "no write ack at 0x{a:08X} (got {other:?}) — the radio rejected \
                     the frame; STOP and restore from the backup"
                ))
            }
        }
    }
    Ok(())
}

/// Base address + length of the channel bank containing 1-based `slot`, and the
/// record's byte offset within that bank.
pub(crate) fn bank_of_slot(slot: usize) -> (u32, usize, usize) {
    let zero = slot - 1;
    let bank = zero / CH_PER_BANK;
    let within = zero % CH_PER_BANK;
    let base = CHANNEL_BASE + (bank as u32) * BANK_STEP;
    (base, CH_PER_BANK * CH_REC_LEN, within * CH_REC_LEN)
}

/// Write one 128-byte channel record by REWRITING ITS ENTIRE BANK: read the whole
/// bank, splice the new record in at its offset, then write the whole bank back as
/// one contiguous sweep. AnyTone flash is erased a sector at a time, so writing an
/// isolated record erases its neighbours in the same sector — the corruption we
/// hit before. Rewriting the whole bank keeps the sector self-consistent, which is
/// how CPS/qdmr do it. Returns the ORIGINAL bank bytes (pre-splice) for backup.
pub(crate) fn write_slot_record(
    p: &mut dyn SerialPort,
    slot: usize,
    record: &[u8],
) -> Result<Vec<u8>, String> {
    if record.len() != CH_REC_LEN {
        return Err(format!(
            "record is {} bytes, expected {CH_REC_LEN}",
            record.len()
        ));
    }
    let (base, bank_len, rec_off) = bank_of_slot(slot);
    let original_bank = read_block(p, base, bank_len)?;
    let mut bank = original_bank.clone();
    bank[rec_off..rec_off + CH_REC_LEN].copy_from_slice(record);
    write_block(p, base, &bank)?;
    Ok(original_bank)
}

/// Send `END` and read the closing ack. Best-effort; callers ignore failures.
pub(crate) fn end_session(p: &mut dyn SerialPort) -> Result<(), String> {
    p.write_all(END).estr()?;
    match read_byte(p)? {
        Some(ACK) => Ok(()),
        other => Err(format!("no end-session ack (got {other:?})")),
    }
}

// ============================================================
// Helpers
// ============================================================

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn ascii(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' })
        .collect()
}

// ============================================================
// Stage 2 decode: channel / zone records
// ============================================================
//
// Channel record = 128 bytes, hardware-confirmed field map (validated against a
// programmed D890UV — see PROJECT_STATE / memory):
//   0x00..0x04  RX freq, big-endian BCD, units of 10 Hz
//   0x04..0x08  TX *offset*, BE BCD ×10 Hz (0 = simplex)
//   0x08        mode/flags byte, MSB-first bit groups (hardware-confirmed against
//               the dmr-tools d890uv v1.05 descriptor + real captures):
//                 bits 7-6 repeater/shift  0 simplex / 1 +shift / 2 -shift
//                 bits 5-4 bandwidth       0 narrow 12.5 kHz / 1 wide 25 kHz
//                 bits 3-2 TX power        0 Low / 1 Medium / 2 High / 3 Turbo
//                 bits 1-0 channel mode    0 FM / 1 DMR / 2 FM+DMR RX / 3 DMR+FM RX
//   0x09        analog tone-enable bits (low nibble): 0x01 RX CTCSS, 0x02 RX DCS,
//               0x04 TX CTCSS, 0x08 TX DCS (0x05 = CTCSS on both, HW-confirmed).
//               Upper bits are talkaround/call-confirm/rx-only/swap (ignored here).
//   0x0A / 0x0B TX / RX CTCSS tone-table index (order per the dmr-tools descriptor;
//               split TX≠RX assignment not yet spot-checked on hardware)
//   0x0C / 0x0E TX / RX DCS code (u16 LE, raw stored value)
//   0x14        DMR contact (talkgroup) index (u16 LE)
//   0x20        DMR color code (0-15)
//   0x21        DMR time slot (0 = TS1, 1 = TS2)
//   0x44        name, 16 chars UTF-16LE, NUL/0xFFFF terminated

pub(crate) const CH_REC_LEN: usize = 0x80;
pub(crate) const CH_PER_BANK: usize = 128;
pub(crate) const CH_RX: usize = 0x00;
pub(crate) const CH_TX_OFFSET: usize = 0x04;
pub(crate) const CH_MODE: usize = 0x08;
pub(crate) const CH_TONE_EN: usize = 0x09;
pub(crate) const CH_TX_CTCSS: usize = 0x0A;
pub(crate) const CH_RX_CTCSS: usize = 0x0B;
pub(crate) const CH_TX_DCS: usize = 0x0C;
pub(crate) const CH_RX_DCS: usize = 0x0E;
pub(crate) const CH_CONTACT: usize = 0x14;
pub(crate) const CH_COLOR_CODE: usize = 0x20;
pub(crate) const CH_TIME_SLOT: usize = 0x21;
pub(crate) const CH_NAME: usize = 0x44;
pub(crate) const CH_NAME_CHARS: usize = 16;
/// Scan-list index byte (dmr-tools descriptor; 0-based, 0xFF = no scan list —
/// matches the 0xFF observed at this offset in real captures of channels with
/// no scan list assigned).
pub(crate) const CH_SCAN_LIST: usize = 0x1B;
/// TX color code (dmr-tools descriptor). CPS keeps it equal to the RX color
/// code at 0x20; the encoder writes both so freshly created slots don't
/// transmit on CC 0.
pub(crate) const CH_TX_COLOR_CODE: usize = 0x43;

/// CTCSS tone table, Hz, in the radio's index order (62.5 Hz = index 0).
/// Confirmed anchors: idx 0x0d=100.0, 0x10=110.9, 0x13=123.0.
/// NOTE: this table KEEPS 150.0 at idx 25 (146.2→150.0→151.4), which DIVERGES
/// from the dmr-tools d890uv descriptor (its enum omits 150.0). A full radio read
/// rendered BOTH 150.0 and 151.4 as distinct tones — only possible if the radio's
/// own index table includes 150.0 — so this table matches the hardware and the
/// descriptor's enum is the one that's wrong. Do not "fix" it to match the xml.
pub(crate) const CTCSS: &[f64] = &[
    62.5, 67.0, 69.3, 71.9, 74.4, 77.0, 79.7, 82.5, 85.4, 88.5, 91.5, 94.8, 97.4, 100.0, 103.5,
    107.2, 110.9, 114.8, 118.8, 123.0, 127.3, 131.8, 136.5, 141.3, 146.2, 150.0, 151.4, 156.7,
    159.8, 162.2, 165.5, 167.9, 171.3, 173.8, 177.3, 179.9, 183.5, 186.2, 189.9, 192.8, 196.6,
    199.5, 203.5, 206.5, 210.7, 218.1, 225.7, 229.1, 233.6, 241.8, 250.3, 254.1,
];

/// Big-endian packed BCD → integer. Each byte is two decimal digits, most
/// significant byte first: `[0x44,0x60,0x00,0x00]` → 44_600_000.
pub(crate) fn bcd_be(b: &[u8]) -> u64 {
    b.iter().fold(0u64, |acc, &byte| {
        acc * 100 + (byte >> 4) as u64 * 10 + (byte & 0x0F) as u64
    })
}

/// A 4-byte BE-BCD frequency field (units of 10 Hz) → MHz.
pub(crate) fn freq_mhz(b: &[u8]) -> f64 {
    bcd_be(b) as f64 / 100_000.0
}

/// Inverse of [`bcd_be`]: pack an integer into `nbytes` of big-endian packed BCD.
/// The low `2*nbytes` decimal digits are kept; higher digits overflow silently.
/// A Stage 3 write primitive used by [`encode_channel_into_record`].
pub(crate) fn bcd_be_encode(mut value: u64, nbytes: usize) -> Vec<u8> {
    let mut out = vec![0u8; nbytes];
    for slot in out.iter_mut().rev() {
        let pair = (value % 100) as u8;
        *slot = ((pair / 10) << 4) | (pair % 10);
        value /= 100;
    }
    out
}

/// Inverse of [`freq_mhz`]: MHz → 4-byte BE-BCD field in units of 10 Hz.
pub(crate) fn freq_encode(mhz: f64) -> [u8; 4] {
    let value = (mhz * 100_000.0).round() as u64;
    let v = bcd_be_encode(value, 4);
    [v[0], v[1], v[2], v[3]]
}

/// Inverse of the mode-byte decoders: assemble the 0x08 flags byte from its
/// MSB-first bit groups. `shift` 0 simplex / 1 +shift / 2 -shift; `wide` = 25 kHz;
/// `power` 0-3 (Low/Med/High/Turbo); `mode` 0-3 (FM/DMR/FM+DMR RX/DMR+FM RX).
pub(crate) fn encode_mode_byte(shift: u8, wide: bool, power: u8, mode: u8) -> u8 {
    ((shift & 0x03) << 6) | ((wide as u8) << 4) | ((power & 0x03) << 2) | (mode & 0x03)
}

/// Decode the 16-char UTF-16LE name field.
pub(crate) fn decode_name(rec: &[u8]) -> String {
    decode_utf16_name(&rec[CH_NAME..], CH_NAME_CHARS)
}

/// Decode a fixed-width UTF-16LE name from the start of `data`, up to `chars`
/// code units, stopping at the first NUL/0xFFFF terminator. Used for both channel
/// names (at an offset) and zone names (a standalone region).
pub(crate) fn decode_utf16_name(data: &[u8], chars: usize) -> String {
    data.chunks_exact(2)
        .take(chars)
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .take_while(|&c| c != 0 && c != 0xFFFF)
        .filter_map(|c| char::from_u32(c as u32))
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// CTCSS tone label for a tone-table index.
pub(crate) fn ctcss_label(idx: u8) -> String {
    CTCSS
        .get(idx as usize)
        .map(|hz| format!("{hz:.1}"))
        .unwrap_or_else(|| format!("idx{idx}"))
}

/// TX power label from the mode byte (bits 3-2).
pub(crate) fn power_label(mode_byte: u8) -> &'static str {
    match (mode_byte >> 2) & 0x03 {
        0 => "Low",
        1 => "Medium",
        2 => "High",
        _ => "Turbo",
    }
}

/// Channel bandwidth label from the mode byte (bits 5-4). Only 0/1 are defined;
/// DMR channels are always narrow.
pub(crate) fn bandwidth_label(mode_byte: u8) -> &'static str {
    if (mode_byte >> 4) & 0x03 == 1 {
        "25 kHz"
    } else {
        "12.5 kHz"
    }
}

/// Channel mode from the mode byte (bits 1-0): the display label plus whether the
/// channel carries a DMR component (so the DMR color-code/slot/contact fields apply).
pub(crate) fn channel_mode(mode_byte: u8) -> (&'static str, bool) {
    match mode_byte & 0x03 {
        0 => ("FM", false),
        1 => ("DMR", true),
        2 => ("FM+DMR RX", true),
        _ => ("DMR+FM RX", true),
    }
}

/// DCS code label from a u16-LE code field. The radio stores the raw binary
/// value and displays it in octal (HW-confirmed: 265 dec shows as D411); the
/// label uses the octal code, matching the radio and the rest of the app.
pub(crate) fn dcs_label(rec: &[u8], off: usize) -> String {
    let code = u16::from_le_bytes([rec[off], rec[off + 1]]);
    format!("DCS {code:03o}")
}

/// One direction's tone from the enable bits: CTCSS wins over DCS if both flagged.
pub(crate) fn tone_side(ctcss_on: bool, dcs_on: bool, rec: &[u8], ctcss_off: usize, dcs_off: usize) -> Option<String> {
    if ctcss_on {
        Some(format!("CTCSS {}", ctcss_label(rec[ctcss_off])))
    } else if dcs_on {
        Some(dcs_label(rec, dcs_off))
    } else {
        None
    }
}

/// Structured TX/RX sub-tones from the enable byte's low nibble, the same fields
/// `decode_tone` renders. CTCSS wins over DCS when both bits are flagged
/// (matching `tone_side`); an off-table CTCSS index decodes as None (the display
/// label still shows it as "idxN").
pub(crate) fn decode_tone_sides(rec: &[u8]) -> (Option<AnytoneSubTone>, Option<AnytoneSubTone>) {
    let en = rec[CH_TONE_EN];
    let side = |ctcss_on: bool, dcs_on: bool, ctcss_off: usize, dcs_off: usize| {
        if ctcss_on {
            CTCSS
                .get(rec[ctcss_off] as usize)
                .copied()
                .map(AnytoneSubTone::Ctcss)
        } else if dcs_on {
            Some(AnytoneSubTone::Dcs(u16::from_le_bytes([
                rec[dcs_off],
                rec[dcs_off + 1],
            ])))
        } else {
            None
        }
    };
    (
        side(en & 0x04 != 0, en & 0x08 != 0, CH_TX_CTCSS, CH_TX_DCS),
        side(en & 0x01 != 0, en & 0x02 != 0, CH_RX_CTCSS, CH_RX_DCS),
    )
}

/// Analog tone summary from the enable byte's low nibble (0x01 RX CTCSS, 0x02 RX
/// DCS, 0x04 TX CTCSS, 0x08 TX DCS) plus the CTCSS/DCS code fields. Collapses to a
/// single label when TX and RX match, else shows both sides.
pub(crate) fn decode_tone(rec: &[u8]) -> String {
    let en = rec[CH_TONE_EN];
    let tx = tone_side(en & 0x04 != 0, en & 0x08 != 0, rec, CH_TX_CTCSS, CH_TX_DCS);
    let rx = tone_side(en & 0x01 != 0, en & 0x02 != 0, rec, CH_RX_CTCSS, CH_RX_DCS);
    match (tx, rx) {
        (None, None) => "—".to_string(),
        (Some(t), Some(r)) if t == r => t,
        (Some(t), Some(r)) => format!("TX {t} / RX {r}"),
        (Some(t), None) => format!("TX {t}"),
        (None, Some(r)) => format!("RX {r}"),
    }
}

/// True for an empty/unprogrammed slot (all 0xFF or all 0x00).
pub(crate) fn is_empty_record(rec: &[u8]) -> bool {
    rec.iter().all(|&b| b == 0xFF) || rec.iter().all(|&b| b == 0x00)
}

/// Decode one 128-byte channel record. `slot` is the 0-based memory slot; the
/// returned `index` is 1-based (CPS numbering). `None` for an empty slot.
pub(crate) fn decode_channel(rec: &[u8], slot: usize) -> Option<AnytoneDecodedChannel> {
    if rec.len() < CH_REC_LEN || is_empty_record(rec) {
        return None;
    }
    let rx_mhz = freq_mhz(&rec[CH_RX..CH_RX + 4]);
    let mode_byte = rec[CH_MODE];
    let (mode_label, digital) = channel_mode(mode_byte);

    let offset = freq_mhz(&rec[CH_TX_OFFSET..CH_TX_OFFSET + 4]);
    let shift = if offset.abs() < 1e-9 {
        String::new() // simplex
    } else if mode_byte & 0x80 != 0 {
        format!("-{offset:.3}")
    } else if mode_byte & 0x40 != 0 {
        format!("+{offset:.3}")
    } else {
        // Offset magnitude set but no direction bit — show unsigned so it's visible.
        format!("{offset:.3}")
    };

    // Actual TX frequency from the offset + direction bits; a set offset with no
    // direction bit is treated as simplex (matching the unsigned `shift` render).
    let tx_mhz = if mode_byte & 0x80 != 0 {
        rx_mhz - offset
    } else if mode_byte & 0x40 != 0 {
        rx_mhz + offset
    } else {
        rx_mhz
    };
    // Round to 10 Hz (the record's BCD resolution) to shed float dust from ±.
    let tx_mhz = (tx_mhz * 100_000.0).round() / 100_000.0;

    let (tone, tone_tx, tone_rx, color_code, time_slot, contact_index) = if digital {
        (
            "—".to_string(),
            None,
            None,
            Some(rec[CH_COLOR_CODE]),
            Some(rec[CH_TIME_SLOT] + 1),
            Some(u16::from_le_bytes([rec[CH_CONTACT], rec[CH_CONTACT + 1]])),
        )
    } else {
        let (tx, rx) = decode_tone_sides(rec);
        (decode_tone(rec), tx, rx, None, None, None)
    };

    Some(AnytoneDecodedChannel {
        index: slot + 1,
        name: decode_name(rec),
        rx_mhz,
        tx_mhz,
        shift,
        mode: mode_label.to_string(),
        power: power_label(mode_byte).to_string(),
        bandwidth: bandwidth_label(mode_byte).to_string(),
        tone,
        tone_tx,
        tone_rx,
        color_code,
        time_slot,
        contact_index,
        contact_name: None, // filled in later from the Contact bank
    })
}

/// Decode every programmed channel from the banks read. `banks` is a list of
/// `(base_slot, bytes)`; bank 0 starts at slot 0, bank 1 at slot 128, etc.
/// Returns the channels plus a 0-based slot → name map for zone resolution.
pub(crate) fn decode_channels(banks: &[(usize, &[u8])]) -> (Vec<AnytoneDecodedChannel>, HashMap<usize, String>) {
    let mut channels = Vec::new();
    let mut names = HashMap::new();
    for &(base, data) in banks {
        for (i, rec) in data.chunks(CH_REC_LEN).enumerate() {
            let slot = base + i;
            if let Some(ch) = decode_channel(rec, slot) {
                names.insert(slot, ch.name.clone());
                channels.push(ch);
            }
        }
    }
    (channels, names)
}

/// Decode a zone's channel list: 16-bit LE, 0-based slot indices,
/// 0xFFFF-terminated. Members are resolved to names where the slot is known.
pub(crate) fn decode_zone(
    data: &[u8],
    names: &HashMap<usize, String>,
    name: String,
    index: usize,
) -> AnytoneDecodedZone {
    let member_slots: Vec<u16> = data
        .chunks_exact(2)
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .take_while(|&idx| idx != 0xFFFF)
        .collect();
    let channels = member_slots
        .iter()
        .map(|&idx| {
            names
                .get(&(idx as usize))
                .cloned()
                .unwrap_or_else(|| format!("#{}", idx + 1))
        })
        .collect();
    AnytoneDecodedZone {
        index,
        name,
        channels,
        member_slots,
    }
}

// ============================================================
// Stage 3 encode — app-model channel → 128-byte record (read-modify-write)
// ============================================================

/// One direction's sub-audible tone in a channel edit. CTCSS carries the tone in
/// Hz (resolved to a table index on encode); DCS carries the RAW stored code
/// (u16) — HW-confirmed: the radio stores raw binary and displays it in octal
/// (265 dec = D411), so octal↔raw conversion happens at the string edges only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum AnytoneSubTone {
    Ctcss(f64),
    Dcs(u16),
}

/// Analog tone edit: independent TX and RX sub-tones, either of which may be
/// absent (no tone on that side).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnytoneToneEdit {
    pub tx: Option<AnytoneSubTone>,
    pub rx: Option<AnytoneSubTone>,
}

/// A structured, app-model channel edit to patch onto an existing record. ONLY the
/// fields modeled here are written; every other byte — including fields we decode
/// but don't yet re-model (e.g. still-unconfirmed layout) — is preserved by the
/// read-modify-write encoder. The channel `mode` bits select which field group
/// applies: analog modes (0) use `tone`; digital modes (1-3) use `color_code` /
/// `time_slot` / `contact_index`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnytoneChannelEdit {
    pub name: String,
    pub rx_mhz: f64,
    /// Absolute TX frequency; the encoder derives the offset magnitude + direction
    /// (simplex when it equals `rx_mhz`).
    pub tx_mhz: f64,
    /// TX power bits (0 Low / 1 Medium / 2 High / 3 Turbo).
    pub power: u8,
    /// Bandwidth: true = 25 kHz (wide), false = 12.5 kHz (narrow).
    pub wide: bool,
    /// Channel-mode bits (0 FM / 1 DMR / 2 FM+DMR RX / 3 DMR+FM RX).
    pub mode: u8,
    #[serde(default)]
    pub tone: AnytoneToneEdit,
    #[serde(default)]
    pub color_code: u8,
    /// 1-based time slot (1 or 2), as the decoder presents it.
    #[serde(default)]
    pub time_slot: u8,
    #[serde(default)]
    pub contact_index: u16,
    /// Scan-list byte (0x1B): `Some(idx)` assigns the 0-based scan list,
    /// `Some(0xFF)` explicitly clears it, `None` preserves the record's
    /// existing byte (so pre-existing edit paths are unchanged).
    #[serde(default)]
    pub scan_list_index: Option<u8>,
}

/// Reverse of [`ctcss_label`]: the CTCSS table index for a tone in Hz, or `None`
/// if no table entry is within 0.05 Hz (the table is spaced ≥0.1 Hz apart).
pub(crate) fn ctcss_index(hz: f64) -> Option<u8> {
    CTCSS
        .iter()
        .position(|&t| (t - hz).abs() < 0.05)
        .map(|i| i as u8)
}

/// Derive the mode-byte shift group (0 simplex / 1 +shift / 2 -shift) and the
/// 4-byte BE-BCD offset-magnitude field from an RX/TX pair — the inverse of the
/// decoder's offset + direction-bit reading.
pub(crate) fn encode_shift(rx_mhz: f64, tx_mhz: f64) -> (u8, [u8; 4]) {
    let diff = tx_mhz - rx_mhz;
    if diff.abs() < 1e-6 {
        (0, [0, 0, 0, 0]) // simplex
    } else if diff < 0.0 {
        (2, freq_encode(diff.abs())) // -shift → mode bit 7
    } else {
        (1, freq_encode(diff)) // +shift → mode bit 6
    }
}

/// Write a fixed-width UTF-16LE name into a record's name field, clearing the full
/// 16-code-unit field first so no stale characters survive (the decoder stops at
/// the first NUL). Inverse of [`decode_name`].
pub(crate) fn encode_name_into(field: &mut [u8], name: &str) {
    let bytes = CH_NAME_CHARS * 2;
    field[..bytes].fill(0);
    for (i, c) in name.encode_utf16().take(CH_NAME_CHARS).enumerate() {
        field[i * 2..i * 2 + 2].copy_from_slice(&c.to_le_bytes());
    }
}

/// Patch the analog tone fields (enable nibble + CTCSS index / DCS code fields) —
/// the inverse of [`decode_tone`]. Rebuilds the enable byte's low nibble from
/// scratch (old tone bits cleared) while preserving its high nibble. CTCSS wins
/// per side if set; a side left `None` clears that direction's tone.
pub(crate) fn encode_tone_into(rec: &mut [u8], tone: &AnytoneToneEdit) -> Result<(), String> {
    let mut en = rec[CH_TONE_EN] & 0xF0; // preserve unknown high-nibble flags
    match &tone.tx {
        Some(AnytoneSubTone::Ctcss(hz)) => {
            en |= 0x04;
            rec[CH_TX_CTCSS] = ctcss_index(*hz)
                .ok_or_else(|| format!("no CTCSS table entry for {hz} Hz (TX)"))?;
        }
        Some(AnytoneSubTone::Dcs(code)) => {
            en |= 0x08;
            rec[CH_TX_DCS..CH_TX_DCS + 2].copy_from_slice(&code.to_le_bytes());
        }
        None => {}
    }
    match &tone.rx {
        Some(AnytoneSubTone::Ctcss(hz)) => {
            en |= 0x01;
            rec[CH_RX_CTCSS] = ctcss_index(*hz)
                .ok_or_else(|| format!("no CTCSS table entry for {hz} Hz (RX)"))?;
        }
        Some(AnytoneSubTone::Dcs(code)) => {
            en |= 0x02;
            rec[CH_RX_DCS..CH_RX_DCS + 2].copy_from_slice(&code.to_le_bytes());
        }
        None => {}
    }
    rec[CH_TONE_EN] = en;
    Ok(())
}

/// Patch an [`AnytoneChannelEdit`] onto an existing 128-byte channel `rec`
/// (read-modify-write): only the modeled bytes are overwritten; all other bytes
/// are left untouched, so unmodeled/still-unconfirmed fields survive verbatim.
/// Pure (no I/O) so it round-trips against the decoders under test. `rec` must be
/// exactly `CH_REC_LEN` bytes.
pub(crate) fn encode_channel_into_record(
    rec: &mut [u8],
    edit: &AnytoneChannelEdit,
) -> Result<(), String> {
    if rec.len() != CH_REC_LEN {
        return Err(format!(
            "record is {} bytes, expected {CH_REC_LEN}",
            rec.len()
        ));
    }
    rec[CH_RX..CH_RX + 4].copy_from_slice(&freq_encode(edit.rx_mhz));
    let (shift, off) = encode_shift(edit.rx_mhz, edit.tx_mhz);
    rec[CH_TX_OFFSET..CH_TX_OFFSET + 4].copy_from_slice(&off);
    rec[CH_MODE] = encode_mode_byte(shift, edit.wide, edit.power, edit.mode);
    encode_name_into(&mut rec[CH_NAME..], &edit.name);
    if let Some(idx) = edit.scan_list_index {
        rec[CH_SCAN_LIST] = idx;
    }

    // Only the field group the channel mode actually uses is written; the other
    // group's bytes are preserved (RMW), mirroring how the decoder ignores them.
    let (_, digital) = channel_mode(rec[CH_MODE]);
    if digital {
        rec[CH_COLOR_CODE] = edit.color_code;
        rec[CH_TX_COLOR_CODE] = edit.color_code;
        rec[CH_TIME_SLOT] = edit.time_slot.saturating_sub(1);
        rec[CH_CONTACT..CH_CONTACT + 2].copy_from_slice(&edit.contact_index.to_le_bytes());
    } else {
        encode_tone_into(rec, &edit.tone)?;
    }
    Ok(())
}

/// One channel write request: which 1-based `slot` to patch, and the edit to apply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnytoneChannelWrite {
    pub slot: usize,
    pub edit: AnytoneChannelEdit,
}

/// A planned per-bank write: the bank base, its ORIGINAL bytes (for backup), and
/// the patched bytes to write. Computed read-only so the backup can be persisted
/// BEFORE any byte is written to the radio.
pub(crate) struct BankPlan {
    pub(crate) base: u32,
    pub(crate) original: Vec<u8>,
    pub(crate) patched: Vec<u8>,
}

/// Phase 1 (read-only): group edits by channel bank, read each affected bank once,
/// and compute the patched bank by read-modify-writing every edited record onto its
/// existing bytes (via [`encode_channel_into_record`]). No writes are issued, so
/// the caller can save a backup of the originals before committing. Range-checks
/// every slot first. Brick-safe granularity (whole-bank).
// Currently unused: its only caller was the retired `write_channels_anytone` RE
// command. Kept because it is the driver's channel-write planner — Chunk 3.6 needs
// it when the AnyTone gains a registry-dispatched channel write path.
#[allow(dead_code)]
pub(crate) fn plan_channel_writes(
    p: &mut dyn SerialPort,
    writes: &[AnytoneChannelWrite],
) -> Result<Vec<BankPlan>, String> {
    let max_slot = NUM_BANKS * CH_PER_BANK;
    for w in writes {
        if w.slot == 0 || w.slot > max_slot {
            return Err(format!("slot {} out of range (1..={max_slot})", w.slot));
        }
    }
    // Group edits by bank base so each bank is read + written exactly once even
    // when several channels in it change together.
    let mut by_bank: std::collections::BTreeMap<u32, Vec<&AnytoneChannelWrite>> =
        std::collections::BTreeMap::new();
    for w in writes {
        let (base, _, _) = bank_of_slot(w.slot);
        by_bank.entry(base).or_default().push(w);
    }

    let bank_len = CH_PER_BANK * CH_REC_LEN;
    let mut plans = Vec::new();
    for (base, group) in by_bank {
        let original = read_block(p, base, bank_len)?;
        let mut patched = original.clone();
        for w in group {
            let (_, _, rec_off) = bank_of_slot(w.slot);
            encode_channel_into_record(&mut patched[rec_off..rec_off + CH_REC_LEN], &w.edit)?;
        }
        plans.push(BankPlan {
            base,
            original,
            patched,
        });
    }
    Ok(plans)
}

/// Phase 2 (writes): write each planned bank back as one contiguous sweep. Returns
/// the bank addresses written. The caller ENDs the session ONCE afterwards so the
/// radio commits + reboots a single time. Does NOT read back (in-session reads come
/// from flash and don't reflect writes — verify in a fresh session after reboot).
pub(crate) fn commit_channel_writes(
    p: &mut dyn SerialPort,
    plans: &[BankPlan],
) -> Result<Vec<u32>, String> {
    let mut written = Vec::new();
    for plan in plans {
        write_block(p, plan.base, &plan.patched)?;
        written.push(plan.base);
    }
    Ok(written)
}

/// Serialize backup banks to the self-describing `[addr:u32 BE][len:u32 BE][data]`
/// dump format (same layout `parse_dump`/`diff_anytone_dumps` consume), so a
/// pre-write backup of every affected bank lands in one alignable file.
pub(crate) fn serialize_backup(banks: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (addr, data) in banks {
        out.extend_from_slice(&addr.to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(data);
    }
    out
}

// ============================================================
// Stage 3b — zone + contact writes (generic 0x4000-window RMW)
// ============================================================

/// The HW-proven safe write unit: writing one isolated record erases its flash
/// sector's neighbours (the session-19 brick), while rewriting a whole 16 KB
/// channel bank was proven collateral-free. Zone/contact regions get the same
/// treatment: every write is a read-modify-write of the full 0x4000-aligned
/// window(s) covering the edit. All region bases are 0x4000-aligned.
pub(crate) const WRITE_WINDOW: usize = CH_PER_BANK * CH_REC_LEN; // 0x4000

/// Radio-side cap on channels per zone (matches `channels_per_zone` in seed.rs;
/// the 0x200 list record could hold 256 entries but the radio UI stops at 160).
pub(crate) const ZONE_MAX_CHANNELS: usize = 160;

/// One absolute byte-range overwrite inside the codeplug address space. Unlike
/// channel edits (whose encoding depends on the record's existing bytes), zone
/// and contact fields are absolute values, so a plain range overwrite suffices.
#[derive(Debug, Clone, PartialEq)]
pub struct RegionPatch {
    pub addr: u32,
    pub data: Vec<u8>,
}

/// Split patches into per-window segments keyed by 0x4000-aligned window base.
/// A patch spanning a window boundary (contact records are 0xC8 and straddle
/// freely) contributes a segment to each window it touches. Pure; unit-tested.
pub(crate) fn split_into_windows(
    patches: &[RegionPatch],
) -> std::collections::BTreeMap<u32, Vec<(usize, Vec<u8>)>> {
    let win = WRITE_WINDOW as u32;
    let mut by_window: std::collections::BTreeMap<u32, Vec<(usize, Vec<u8>)>> =
        std::collections::BTreeMap::new();
    for patch in patches {
        let mut addr = patch.addr;
        let mut data = &patch.data[..];
        while !data.is_empty() {
            let base = addr & !(win - 1);
            let off = (addr - base) as usize;
            let take = data.len().min(WRITE_WINDOW - off);
            by_window
                .entry(base)
                .or_default()
                .push((off, data[..take].to_vec()));
            addr += take as u32;
            data = &data[take..];
        }
    }
    by_window
}

/// Phase 1 (read-only): read each affected 0x4000 window once and splice every
/// patch segment onto its original bytes. No writes are issued.
pub(crate) fn plan_patch_writes(
    p: &mut dyn SerialPort,
    patches: &[RegionPatch],
) -> Result<Vec<BankPlan>, String> {
    let mut plans = Vec::new();
    for (base, segs) in split_into_windows(patches) {
        let original = read_block(p, base, WRITE_WINDOW)?;
        let mut patched = original.clone();
        for (off, data) in segs {
            patched[off..off + data.len()].copy_from_slice(&data);
        }
        plans.push(BankPlan {
            base,
            original,
            patched,
        });
    }
    Ok(plans)
}

/// Outcome of a committed patch write (zones/contacts).
#[derive(Serialize)]
pub struct AnytonePatchWriteResult {
    /// 0x4000 window base addresses actually rewritten (hex).
    pub windows_written: Vec<String>,
    /// Absolute path of the mandatory pre-write backup of every affected window.
    pub backup_path: String,
    /// Human note on how to verify (fresh-session read-back after the reboot).
    pub note: String,
}

/// Blocking runner for a batch of absolute patches: PROGRAM+ident → plan (read
/// windows, splice — no writes) → backup ORIGINAL windows to `backup_path`
/// BEFORE any byte is written → write each whole window back → single END
/// (commit + reboot). Shared by the Tauri commands and the CLI examples, same
/// discipline as `write_channels_anytone`. Brick-capable; keep gated.
pub fn run_patch_writes(
    port: &str,
    patches: &[RegionPatch],
    backup_path: &std::path::Path,
) -> Result<AnytonePatchWriteResult, String> {
    if patches.is_empty() {
        return Err("no patches supplied".to_string());
    }
    let mut p = open_port(port)?;
    let _ident = enter_program_and_ident(&mut *p)?;

    let plans = match plan_patch_writes(&mut *p, patches) {
        Ok(v) => v,
        Err(e) => {
            let _ = end_session(&mut *p);
            return Err(e);
        }
    };
    let backups: Vec<(u32, Vec<u8>)> =
        plans.iter().map(|pl| (pl.base, pl.original.clone())).collect();
    std::fs::write(backup_path, serialize_backup(&backups)).map_err(|e| {
        let _ = end_session(&mut *p);
        format!("could not write backup {}: {e}", backup_path.display())
    })?;

    let written = commit_channel_writes(&mut *p, &plans)?;
    let _ = end_session(&mut *p);

    Ok(AnytonePatchWriteResult {
        windows_written: written.iter().map(|a| format!("0x{a:08X}")).collect(),
        backup_path: backup_path.to_string_lossy().to_string(),
        note: "Written and committed (radio rebooted). Verify in a fresh session \
               after USB re-enumerates."
            .to_string(),
    })
}

/// Write patches DIRECTLY — no read-modify-write — one `write_block` per patch
/// at its absolute address (data padded up to the 16-byte frame size with 0xFF),
/// then a single END (commit + reboot).
///
/// This exists because the RMW path ([`run_patch_writes`], which reads every
/// affected 0x4000 window before writing) DROPS interior flash banks when the
/// total write is large: the long read phase degrades the radio's commit so
/// only the first/last banks survive (HW-proven — a 100k/300k caller-ID DB came
/// back with its middle map+DB banks empty via RMW, but fully intact via this
/// direct path). Use ONLY for regions we FULLY overwrite (caller-ID DB), where
/// no existing bytes need preserving so the RMW read is unnecessary. No backup
/// is taken (the read to make one is the very thing that breaks the commit);
/// the region is self-contained and regenerable. Brick-capable — keep gated.
pub fn run_patch_writes_direct(
    port: &str,
    patches: &[RegionPatch],
) -> Result<AnytonePatchWriteResult, String> {
    if patches.is_empty() {
        return Err("no patches supplied".to_string());
    }
    let mut p = open_port(port)?;
    let _ident = enter_program_and_ident(&mut *p)?;
    let mut written = Vec::new();
    for patch in patches {
        let mut data = patch.data.clone();
        while data.len() % (READ_FRAME as usize) != 0 {
            data.push(0xFF);
        }
        if let Err(e) = write_block(&mut *p, patch.addr, &data) {
            let _ = end_session(&mut *p);
            return Err(e);
        }
        written.push(patch.addr);
    }
    let _ = end_session(&mut *p);
    Ok(AnytonePatchWriteResult {
        windows_written: written.iter().map(|a| format!("0x{a:08X}")).collect(),
        backup_path: String::new(),
        note: "Written and committed (radio rebooted). Verify on the radio's \
               caller-ID screen after USB re-enumerates."
            .to_string(),
    })
}

/// Zero-padded fixed-width UTF-16LE name field (`chars` UTF-16 units). HW note:
/// CPS leaves stale bytes after the NUL terminator; zeroing the whole field
/// decodes identically (`decode_utf16_name` stops at the first NUL) and is clean.
pub(crate) fn encode_utf16_field(name: &str, chars: usize) -> Vec<u8> {
    let mut out = vec![0u8; chars * 2];
    for (i, c) in name.encode_utf16().take(chars).enumerate() {
        out[i * 2..i * 2 + 2].copy_from_slice(&c.to_le_bytes());
    }
    out
}

/// Full 0x200 zone channel-list record: 0-based u16 LE channel indices then
/// solid 0xFF fill (exactly the layout captured from the radio — the 0xFFFF
/// list terminator is just the first two fill bytes).
pub(crate) fn encode_zone_list_record(indices: &[u16]) -> Result<Vec<u8>, String> {
    if indices.len() > ZONE_MAX_CHANNELS {
        return Err(format!(
            "zone has {} channels (max {ZONE_MAX_CHANNELS})",
            indices.len()
        ));
    }
    let max_idx = (NUM_BANKS * CH_PER_BANK) as u16;
    let mut out = vec![0xFFu8; ZONE_LIST_STEP as usize];
    for (i, &idx) in indices.iter().enumerate() {
        if idx >= max_idx {
            return Err(format!("channel index {idx} out of range (0..{max_idx})"));
        }
        out[i * 2..i * 2 + 2].copy_from_slice(&idx.to_le_bytes());
    }
    Ok(out)
}

/// Full 0x200 scan-list record (descriptor-sourced layout, see the
/// `SCAN_LIST_BASE` doc). Priority scanning off, priority indices none
/// (0xFFFF), CPS-default timers (look-back 2.0/3.0 s, dropout 3.1 s, dwell
/// 3.1 s in 0.1-s units), revert = Selected Channel, unset member entries
/// 0xFFFF, zero-filled tail. `indices` are 0-based channel slot indices.
/// The eight editable scan-list settings, resolved to the radio's encoding
/// (priority channels as 0-based radio slots, timers in 0.1-s units). Built by
/// the program planner from the DB row; consumed by `encode_scan_list_record`.
///
/// NOTE: `revert_channel` (@0xF8) is HW-pinned (s53, AnyTone-CPS-oracle diffs:
/// Last Called=4, Last Used=5 — qdmr's value table AND offset confirmed; s51's
/// 0x94 was an RT Systems layout bug). `priority_select` (@0x01) is HW-pinned
/// for 0=Off and 1=Ch1 (same s53 diffs); 2=Ch2 and 3=Ch1+Ch2 remain
/// qdmr-sourced.
pub struct ScanListRecordSettings {
    pub priority_select: u8,          // 0=Off,1=Ch1,2=Ch2,3=Ch1+Ch2
    pub priority_slot_1: Option<u16>, // 0-based radio slot; None => 0xFFFF (unset)
    pub priority_slot_2: Option<u16>,
    pub look_back_a: u16, // 0.1-s units
    pub look_back_b: u16,
    pub dropout_delay: u16,
    pub dwell_time: u16,
    pub revert_channel: u8, // 0..=7
}

impl Default for ScanListRecordSettings {
    /// AnyTone CPS defaults (the values the encoder used to hardcode).
    fn default() -> Self {
        Self {
            priority_select: 0,
            priority_slot_1: None,
            priority_slot_2: None,
            look_back_a: 20, // 2.0 s
            look_back_b: 30, // 3.0 s
            dropout_delay: 31, // 3.1 s
            dwell_time: 31,  // 3.1 s
            revert_channel: 0, // Selected Channel
        }
    }
}

/// Encode a priority slot to the record's 1-based index (0 = unset/none).
pub(crate) fn priority_index(slot: Option<u16>) -> u16 {
    slot.map(|s| s + 1).unwrap_or(0xFFFF)
}

pub fn encode_scan_list_record(
    name: &str,
    indices: &[u16],
    settings: &ScanListRecordSettings,
) -> Result<Vec<u8>, String> {
    if name.trim().is_empty() {
        return Err("scan list name is empty".into());
    }
    if indices.len() > SCAN_MAX_CHANNELS {
        return Err(format!(
            "scan list has {} channels (max {SCAN_MAX_CHANNELS})",
            indices.len()
        ));
    }
    if settings.priority_select > 3 {
        return Err(format!(
            "priority_select {} out of range (0..=3)",
            settings.priority_select
        ));
    }
    if settings.revert_channel > 7 {
        return Err(format!(
            "revert_channel {} out of range (0..=7)",
            settings.revert_channel
        ));
    }
    let max_idx = (NUM_BANKS * CH_PER_BANK) as u16;
    for (label, slot) in [
        ("priority channel 1", settings.priority_slot_1),
        ("priority channel 2", settings.priority_slot_2),
    ] {
        if let Some(s) = slot {
            if s >= max_idx {
                return Err(format!("{label} slot {s} out of range (0..{max_idx})"));
            }
        }
    }
    let mut out = vec![0u8; SCAN_LIST_STEP as usize];
    // Priority Channel Select @0x01, priority indices @0x02/@0x04 (1-based; 0xFFFF=none).
    out[0x01] = settings.priority_select;
    out[0x02..0x04].copy_from_slice(&priority_index(settings.priority_slot_1).to_le_bytes());
    out[0x04..0x06].copy_from_slice(&priority_index(settings.priority_slot_2).to_le_bytes());
    // Timers in 0.1-s units: look-back A/B, dropout, dwell.
    out[0x06..0x08].copy_from_slice(&settings.look_back_a.to_le_bytes());
    out[0x08..0x0A].copy_from_slice(&settings.look_back_b.to_le_bytes());
    out[0x0A..0x0C].copy_from_slice(&settings.dropout_delay.to_le_bytes());
    out[0x0C..0x0E].copy_from_slice(&settings.dwell_time.to_le_bytes());
    let name_field = encode_utf16_field(name, SCAN_NAME_CHARS);
    out[SCAN_NAME..SCAN_NAME + name_field.len()].copy_from_slice(&name_field);
    for i in 0..SCAN_MEMBER_SLOTS {
        let off = SCAN_MEMBERS + i * 2;
        out[off..off + 2].copy_from_slice(&0xFFFFu16.to_le_bytes());
    }
    for (i, &idx) in indices.iter().enumerate() {
        if idx >= max_idx {
            return Err(format!("channel index {idx} out of range (0..{max_idx})"));
        }
        let off = SCAN_MEMBERS + i * 2;
        out[off..off + 2].copy_from_slice(&idx.to_le_bytes());
    }
    // Revert Channel Type @0xF8 (right after the member array); tail stays zero.
    out[SCAN_REVERT] = settings.revert_channel;
    Ok(out)
}

/// Full 0xC8 contact record: call type @0x00, call alert None @0x01, DMR ID
/// (4-byte BE BCD) @0x02, 16-char UTF-16LE name @0x06, zero-filled tail (per
/// the dmr-tools descriptor and the HW-confirmed field offsets).
pub(crate) fn encode_contact_record(
    call_type: u8,
    dmr_id: u32,
    name: &str,
) -> Result<Vec<u8>, String> {
    if call_type > 2 {
        return Err(format!("contact call_type {call_type} invalid (0..=2)"));
    }
    if dmr_id > 99_999_999 {
        return Err(format!("contact DMR ID {dmr_id} exceeds 8 digits"));
    }
    if name.trim().is_empty() {
        return Err("contact name is empty".into());
    }
    let mut out = vec![0u8; CONTACT_REC_LEN];
    out[CONTACT_CALL_TYPE] = call_type;
    out[CONTACT_DMR_ID..CONTACT_DMR_ID + 4].copy_from_slice(&bcd_be_encode(dmr_id as u64, 4));
    let name_field = encode_utf16_field(name, CONTACT_NAME_CHARS);
    out[CONTACT_NAME..CONTACT_NAME + name_field.len()].copy_from_slice(&name_field);
    Ok(out)
}

/// Base record for a channel written into a previously-EMPTY slot: descriptor
/// defaults for every unmodeled field, anchored to the bytes real HW captures
/// show at those offsets, so [`encode_channel_into_record`] can patch the
/// modeled fields on top exactly as it does on an existing record.
/// Capture-anchored defaults: DCS code fields carry the CPS default 0x0011,
/// custom-CTCSS @0x10 the CPS default 0x09CF (251.1 Hz), scan-list @0x1B and
/// RX-group-list @0x1C both 0xFF (= none); everything else zero.
pub(crate) fn new_channel_record_template() -> Vec<u8> {
    let mut rec = vec![0u8; CH_REC_LEN];
    rec[CH_TX_DCS..CH_TX_DCS + 2].copy_from_slice(&[0x11, 0x00]);
    rec[CH_RX_DCS..CH_RX_DCS + 2].copy_from_slice(&[0x11, 0x00]);
    rec[0x10..0x12].copy_from_slice(&[0xCF, 0x09]);
    rec[CH_SCAN_LIST] = 0xFF;
    rec[0x1C] = 0xFF; // RX Group List index: none
    rec
}

/// One zone edit: rename and/or replace the zone's channel list. `None` fields
/// are left untouched on the radio. Channel indices are 0-based slot indices
/// (slot N in the CPS = index N-1), matching the stored format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnytoneZoneEdit {
    pub name: Option<String>,
    pub channel_indices: Option<Vec<u16>>,
}

/// A zone edit addressed to a 1-based zone number.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnytoneZoneWrite {
    pub zone: usize,
    pub edit: AnytoneZoneEdit,
}

/// Build the absolute patches for a batch of zone edits. Pure; unit-tested.
pub fn zone_write_patches(writes: &[AnytoneZoneWrite]) -> Result<Vec<RegionPatch>, String> {
    let mut patches = Vec::new();
    for w in writes {
        if w.zone == 0 || w.zone > MAX_ZONES {
            return Err(format!("zone {} out of range (1..={MAX_ZONES})", w.zone));
        }
        if w.edit.name.is_none() && w.edit.channel_indices.is_none() {
            return Err(format!("zone {} edit is empty", w.zone));
        }
        let i = (w.zone - 1) as u32;
        if let Some(name) = &w.edit.name {
            if name.trim().is_empty() {
                return Err(format!("zone {} name is empty", w.zone));
            }
            patches.push(RegionPatch {
                addr: ZONE_NAME_BASE + i * ZONE_NAME_STEP,
                data: encode_utf16_field(name, ZONE_NAME_CHARS),
            });
        }
        if let Some(indices) = &w.edit.channel_indices {
            patches.push(RegionPatch {
                addr: ZONE_LIST_BASE + i * ZONE_LIST_STEP,
                data: encode_zone_list_record(indices)?,
            });
        }
    }
    Ok(patches)
}

/// One contact edit: any subset of the modeled fields (call type 0=Private/
/// 1=Group/2=All, call alert, DMR ID, name). `None` fields keep the radio's
/// existing bytes; unmodeled record bytes are never touched (window RMW).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnytoneContactEdit {
    pub call_type: Option<u8>,
    pub call_alert: Option<u8>,
    pub dmr_id: Option<u32>,
    pub name: Option<String>,
}

/// A contact edit addressed to a 0-based contact index (the same index channel
/// records reference at 0x14).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnytoneContactWrite {
    pub index: usize,
    pub edit: AnytoneContactEdit,
}

/// Build the absolute patches for a batch of contact edits. Field offsets are
/// HW-confirmed: call type @0x00, call alert @0x01, DMR ID 4-byte BE BCD @0x02,
/// 16-char UTF-16LE name @0x06. Pure; unit-tested.
pub fn contact_write_patches(
    writes: &[AnytoneContactWrite],
) -> Result<Vec<RegionPatch>, String> {
    let mut patches = Vec::new();
    for w in writes {
        if w.index >= MAX_CONTACTS_READ {
            return Err(format!(
                "contact index {} out of range (0..{MAX_CONTACTS_READ})",
                w.index
            ));
        }
        let base = CONTACT_BASE + (w.index * CONTACT_REC_LEN) as u32;
        let mut any = false;
        if let Some(ct) = w.edit.call_type {
            if ct > 2 {
                return Err(format!("contact {} call_type {ct} invalid (0..=2)", w.index));
            }
            patches.push(RegionPatch {
                addr: base,
                data: vec![ct],
            });
            any = true;
        }
        if let Some(alert) = w.edit.call_alert {
            patches.push(RegionPatch {
                addr: base + 1,
                data: vec![alert],
            });
            any = true;
        }
        if let Some(id) = w.edit.dmr_id {
            if id > 99_999_999 {
                return Err(format!("contact {} DMR ID {id} exceeds 8 digits", w.index));
            }
            patches.push(RegionPatch {
                addr: base + 2,
                data: bcd_be_encode(id as u64, 4),
            });
            any = true;
        }
        if let Some(name) = &w.edit.name {
            if name.trim().is_empty() {
                return Err(format!("contact {} name is empty", w.index));
            }
            patches.push(RegionPatch {
                addr: base + CONTACT_NAME as u32,
                data: encode_utf16_field(name, CONTACT_NAME_CHARS),
            });
            any = true;
        }
        if !any {
            return Err(format!("contact {} edit is empty", w.index));
        }
    }
    Ok(patches)
}

/// Read a 0x4000 window, back it up, write the identical bytes back, END
/// (commit + reboot). The safe first HW exercise for a new region — proves the
/// window write is sector-collateral-free before any real edit. Returns the
/// original window bytes. Brick-capable; CLI/gated use only.
pub fn run_noop_window(port: &str, base: u32) -> Result<Vec<u8>, String> {
    if base as usize % WRITE_WINDOW != 0 {
        return Err(format!("0x{base:08X} is not 0x4000-aligned"));
    }
    let mut p = open_port(port)?;
    let _ident = enter_program_and_ident(&mut *p)?;
    let original = match read_block(&mut *p, base, WRITE_WINDOW) {
        Ok(v) => v,
        Err(e) => {
            let _ = end_session(&mut *p);
            return Err(e);
        }
    };
    if let Err(e) = write_block(&mut *p, base, &original) {
        let _ = end_session(&mut *p);
        return Err(e);
    }
    let _ = end_session(&mut *p);
    Ok(original)
}


/// Splice `patches` onto pre-read windows, producing one `BankPlan` per window
/// that actually changes. Every patched window must be present in `originals`
/// (the session read it earlier) — a missing window is a logic error, not a
/// hardware condition.
pub(crate) fn plans_from_pre_read(
    originals: &std::collections::BTreeMap<u32, Vec<u8>>,
    patches: &[RegionPatch],
) -> Result<Vec<BankPlan>, String> {
    let mut plans = Vec::new();
    for (base, segments) in split_into_windows(patches) {
        let original = originals
            .get(&base)
            .ok_or_else(|| format!("internal: window 0x{base:08X} was not read"))?;
        let mut patched = original.clone();
        for (off, data) in segments {
            patched[off..off + data.len()].copy_from_slice(&data);
        }
        if patched != *original {
            plans.push(BankPlan {
                base,
                original: original.clone(),
                patched,
            });
        }
    }
    Ok(plans)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_derive_settings_callsign_export_and_diagnostics() {
        use crate::radios::driver::DriverCapabilities;
        let caps = DriverCapabilities::of(&DRIVER);
        // Advertised now (Chunk 3.5):
        assert!(caps.program_settings);
        assert!(caps.write_callsign_db);
        assert!(caps.export);
        assert!(caps.diagnostics);
        // The DB-driven full-codeplug program moved into the driver in 3.6b.
        assert!(caps.program_codeplug);
        // Not applicable: this radio's programming unit is the whole codeplug,
        // not channels alone, and it is not an image-clone radio. `program_image`
        // staying false is what makes `download_image` reject it by name rather
        // than opening the port (see `commands/program.rs`, 3.6c).
        assert!(!caps.write_channels);
        assert!(!caps.program_image);
        assert_eq!(DRIVER.key(), "anytone_atd890uv");
        assert_eq!(DRIVER.display_name(), "AnyTone AT-D890UV");
    }

    #[test]
    fn checksum_sums_address_len_and_data() {
        let addr = [0x00, 0x01, 0x00, 0x10];
        // addr(0x01+0x10) + len(3) + data(1+2+3) = 0x1A.
        assert_eq!(anytone_checksum(&addr, &[1, 2, 3]), 0x1A);
        // Wraps at 256: addr(0xFF) + len(1) + data(0x02) = 0x102 -> 0x02.
        assert_eq!(anytone_checksum(&[0, 0, 0, 0xFF], &[0x02]), 0x02);
    }

    #[test]
    fn write_frame_has_command_addr_len_data_and_checksum() {
        let frame = build_write_frame(0x0100_0000, &[0xAA, 0xBB]);
        // W + 4 addr + len + 2 data + checksum + trailing 0x06 = 10 bytes.
        assert_eq!(frame.len(), 10);
        assert_eq!(frame[0], b'W');
        assert_eq!(&frame[1..5], &[0x01, 0x00, 0x00, 0x00]); // BE32 address
        assert_eq!(frame[5], 0x02); // length
        assert_eq!(&frame[6..8], &[0xAA, 0xBB]); // data
        // checksum = addr(0x01) + len(0x02) + data(0xAA+0xBB=0x165) = 0x168 -> 0x68.
        assert_eq!(frame[8], 0x68);
        assert_eq!(frame[8], anytone_checksum(&0x0100_0000u32.to_be_bytes(), &[0xAA, 0xBB]));
        assert_eq!(frame[9], ACK); // trailing 0x06 terminator (part of the sent frame)
    }

    #[test]
    fn dump_round_trips_through_parse() {
        // Build a dump with two blocks and confirm parse recovers them.
        let mut file = Vec::new();
        for (addr, data) in [(0x0100_0000u32, vec![1u8, 2, 3]), (0x0350_0000, vec![9, 8])] {
            file.extend_from_slice(&addr.to_be_bytes());
            file.extend_from_slice(&(data.len() as u32).to_be_bytes());
            file.extend_from_slice(&data);
        }
        let blocks = parse_dump(&file).unwrap();
        assert_eq!(blocks, vec![(0x0100_0000, vec![1, 2, 3]), (0x0350_0000, vec![9, 8])]);
        // A truncated header/body is rejected, not silently accepted.
        assert!(parse_dump(&file[..file.len() - 1]).is_err());
    }

    #[test]
    fn diff_reports_changed_runs_at_real_addresses() {
        let a = vec![
            (0x0100_0000u32, vec![0x10, 0x20, 0x30, 0x40, 0x50]),
            (0x0350_0000, vec![0xAA, 0xBB, 0xCC]),
        ];
        // b: byte at +1 changed in block 0; a 2-byte run changed in block 1.
        let b = vec![
            (0x0100_0000u32, vec![0x10, 0x21, 0x30, 0x40, 0x50]),
            (0x0350_0000, vec![0xAA, 0xFE, 0xFF]),
        ];
        let diffs = diff_dumps(&a, &b);
        assert_eq!(diffs.len(), 2);
        assert_eq!(diffs[0].addr, "0x01000001");
        assert_eq!(diffs[0].len, 1);
        assert_eq!(diffs[0].a_hex, "20");
        assert_eq!(diffs[0].b_hex, "21");
        assert_eq!(diffs[1].addr, "0x03500001");
        assert_eq!(diffs[1].len, 2);
        assert_eq!(diffs[1].a_hex, "BB CC");
        assert_eq!(diffs[1].b_hex, "FE FF");
        // Identical dumps → no diffs.
        assert!(diff_dumps(&a, &a).is_empty());
    }

    #[test]
    fn slot_addr_maps_slots_into_banks() {
        // Slot 1 = bank 0, record 0.
        assert_eq!(slot_addr(1), CHANNEL_BASE);
        // Slot 120 (FOCOBUCK is slot 119..120 range) = bank 0, record 119.
        assert_eq!(slot_addr(120), CHANNEL_BASE + 119 * CH_REC_LEN as u32);
        // Last record of bank 0.
        assert_eq!(slot_addr(128), CHANNEL_BASE + 127 * CH_REC_LEN as u32);
        // First record of bank 1.
        assert_eq!(slot_addr(129), CHANNEL_BASE + BANK_STEP);
        // First record of bank 2.
        assert_eq!(slot_addr(257), CHANNEL_BASE + 2 * BANK_STEP);
    }

    #[test]
    fn validate_read_header_accepts_a_good_header() {
        let a = 0x0250_0000u32.to_be_bytes();
        let header = [b'W', a[0], a[1], a[2], a[3], 0x10];
        assert!(validate_read_header(0x0250_0000, 0x10, &header).is_ok());
    }

    #[test]
    fn validate_read_header_rejects_wrong_address_command_and_size() {
        let a = 0x0250_0000u32.to_be_bytes();
        let good = [b'W', a[0], a[1], a[2], a[3], 0x10];
        // Wrong echoed address.
        assert!(validate_read_header(0x0250_0001, 0x10, &good).is_err());
        // Wrong command byte.
        let mut bad_cmd = good;
        bad_cmd[0] = b'R';
        assert!(validate_read_header(0x0250_0000, 0x10, &bad_cmd).is_err());
        // Wrong size echo.
        assert!(validate_read_header(0x0250_0000, 0x20, &good).is_err());
        // Truncated header.
        assert!(validate_read_header(0x0250_0000, 0x10, &good[..4]).is_err());
    }

    #[test]
    fn ident_match_requires_the_890_token() {
        assert!(ident_is_d890(b"ID890UV"));
        assert!(ident_is_d890(b"ID890UV..V100"));
        assert!(!ident_is_d890(b"ID878UV"));
        assert!(!ident_is_d890(b"ID578UV"));
    }

    // --- Stage 2 decode -------------------------------------------------

    /// Build a 128-byte channel record from a header prefix + a name.
    fn make_record(header: &[u8], name: &str) -> Vec<u8> {
        let mut r = vec![0u8; CH_REC_LEN];
        r[..header.len()].copy_from_slice(header);
        for (i, c) in name.encode_utf16().take(CH_NAME_CHARS).enumerate() {
            r[CH_NAME + i * 2..CH_NAME + i * 2 + 2].copy_from_slice(&c.to_le_bytes());
        }
        r
    }

    #[test]
    fn bcd_be_and_freq() {
        assert_eq!(bcd_be(&[0x44, 0x60, 0x00, 0x00]), 44_600_000);
        assert_eq!(bcd_be(&[0x00, 0x50, 0x00, 0x00]), 500_000);
        assert!((freq_mhz(&[0x44, 0x60, 0x00, 0x00]) - 446.0).abs() < 1e-9);
        assert!((freq_mhz(&[0x44, 0x52, 0x00, 0x00]) - 445.2).abs() < 1e-9);
    }

    #[test]
    fn bcd_and_freq_encoders_round_trip_real_captures() {
        // Encoders are the exact inverse of the decoders on real captured fields.
        for bytes in [
            [0x44u8, 0x60, 0x00, 0x00], // 446.000 RX (SCAN NC)
            [0x44, 0x52, 0x00, 0x00],   // 445.200 RX (FOCOBUCK)
            [0x00, 0x50, 0x00, 0x00],   // 5.000 offset
            [0x00, 0x00, 0x00, 0x00],   // simplex
        ] {
            assert_eq!(bcd_be_encode(bcd_be(&bytes), 4), bytes);
            assert_eq!(freq_encode(freq_mhz(&bytes)), bytes);
        }
    }

    #[test]
    fn mode_byte_encoder_reproduces_real_captures() {
        // Reassemble each captured mode byte from its decoded bit groups.
        // FOCOBUCK 0x8d: -shift, narrow, Turbo, DMR.
        assert_eq!(encode_mode_byte(2, false, 3, 1), 0x8d);
        // RMH721 0x4d: +shift, narrow, Turbo, DMR.
        assert_eq!(encode_mode_byte(1, false, 3, 1), 0x4d);
        // SCAN NC 0x1c: simplex, wide, Turbo, FM.
        assert_eq!(encode_mode_byte(0, true, 3, 0), 0x1c);
    }

    #[test]
    fn decodes_real_dmr_channel_focobuck() {
        // Exact captured header bytes 0x00..0x22 of slot 118 "RMH700-FOCOBUCK".
        let header = [
            0x44, 0x52, 0x00, 0x00, 0x00, 0x50, 0x00, 0x00, 0x8d, 0x00, 0x00, 0x00, 0x11, 0x00,
            0x11, 0x00, 0xcf, 0x09, 0x00, 0x00, 0x17, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff,
            0xff, 0x00, 0x00, 0x00, 0x07, 0x00,
        ];
        let rec = make_record(&header, "RMH700-FOCOBUCK");
        let ch = decode_channel(&rec, 117).expect("programmed");
        assert_eq!(ch.index, 118); // 1-based
        assert_eq!(ch.name, "RMH700-FOCOBUCK");
        assert!((ch.rx_mhz - 445.2).abs() < 1e-9);
        assert_eq!(ch.shift, "-5.000"); // 445.2 - 5.0 = 440.2 TX
        assert!((ch.tx_mhz - 440.2).abs() < 1e-9);
        assert_eq!((ch.tone_tx, ch.tone_rx), (None, None)); // digital: no analog tones
        assert_eq!(ch.mode, "DMR");
        assert_eq!(ch.power, "Turbo"); // 0x8d bits 3-2 = 11
        assert_eq!(ch.bandwidth, "12.5 kHz"); // 0x8d bits 5-4 = 00 (DMR = narrow)
        assert_eq!(ch.color_code, Some(7));
        assert_eq!(ch.time_slot, Some(1)); // byte 0x21 = 0x00
        assert_eq!(ch.contact_index, Some(23)); // byte 0x14 = 0x17
        assert_eq!(ch.tone, "—");
    }

    #[test]
    fn decodes_ts2_and_plus_shift() {
        // RMH721-FOCOUHF: TS2 (0x21=0x01). Synthesize a + shift via mode bit6.
        let header = [
            0x44, 0x52, 0x00, 0x00, 0x00, 0x50, 0x00, 0x00, 0x4d, 0x00, 0x00, 0x00, 0x11, 0x00,
            0x11, 0x00, 0xcf, 0x09, 0x00, 0x00, 0x21, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff,
            0xff, 0x00, 0x00, 0x00, 0x07, 0x01,
        ];
        let ch = decode_channel(&make_record(&header, "RMH721-FOCOUHF"), 160).unwrap();
        assert_eq!(ch.time_slot, Some(2)); // byte 0x21 = 0x01
        assert_eq!(ch.shift, "+5.000"); // mode bit6 set
        assert!((ch.tx_mhz - 450.2).abs() < 1e-9);
        assert_eq!(ch.contact_index, Some(33)); // byte 0x14 = 0x21
    }

    #[test]
    fn decodes_real_analog_channel() {
        // Exact captured header bytes of slot 1 "SCAN NC DMRGMRS" (CTCSS 100.0).
        let header = [
            0x44, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1c, 0x05, 0x0d, 0x0d, 0x11, 0x00,
            0x11, 0x00,
        ];
        let ch = decode_channel(&make_record(&header, "SCAN NC DMRGMRS"), 1).unwrap();
        assert_eq!(ch.name, "SCAN NC DMRGMRS");
        assert!((ch.rx_mhz - 446.0).abs() < 1e-9);
        assert_eq!(ch.shift, ""); // simplex
        assert!((ch.tx_mhz - 446.0).abs() < 1e-9); // simplex: TX == RX
        assert_eq!(ch.mode, "FM");
        assert_eq!(ch.power, "Turbo"); // 0x1c bits 3-2 = 11
        assert_eq!(ch.bandwidth, "25 kHz"); // 0x1c bits 5-4 = 01 (wide)
        assert_eq!(ch.tone, "CTCSS 100.0");
        assert_eq!(ch.tone_tx, Some(AnytoneSubTone::Ctcss(100.0)));
        assert_eq!(ch.tone_rx, Some(AnytoneSubTone::Ctcss(100.0)));
        assert_eq!(ch.color_code, None);
        assert_eq!(ch.time_slot, None);
    }

    #[test]
    fn decodes_power_bandwidth_and_channel_mode_bits() {
        // Synthesize the four power levels and both bandwidths via the mode byte.
        // Layout MSB-first: [shift:2][bw:2][power:2][mode:2].
        let base = [
            0x44, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ];
        let with_mode = |m: u8| {
            let mut h = base;
            h[CH_MODE] = m;
            decode_channel(&make_record(&h, "X"), 0).unwrap()
        };
        // power bits 3-2
        assert_eq!(with_mode(0b00_00_00_00).power, "Low");
        assert_eq!(with_mode(0b00_00_01_00).power, "Medium");
        assert_eq!(with_mode(0b00_00_10_00).power, "High");
        assert_eq!(with_mode(0b00_00_11_00).power, "Turbo");
        // bandwidth bits 5-4
        assert_eq!(with_mode(0b00_00_00_00).bandwidth, "12.5 kHz");
        assert_eq!(with_mode(0b00_01_00_00).bandwidth, "25 kHz");
        // channel-mode bits 1-0
        assert_eq!(with_mode(0b00_00_00_00).mode, "FM");
        assert_eq!(with_mode(0b00_00_00_01).mode, "DMR");
        assert_eq!(with_mode(0b00_00_00_10).mode, "FM+DMR RX");
        assert_eq!(with_mode(0b00_00_00_11).mode, "DMR+FM RX");
        // FM+DMR RX / DMR+FM RX carry DMR fields.
        assert!(with_mode(0b00_00_00_10).color_code.is_some());
        assert!(with_mode(0b00_00_00_11).time_slot.is_some());
    }

    #[test]
    fn decodes_split_ctcss_and_dcs_tone_modes() {
        // FM record; drive the tone-enable low nibble + tone/DCS code fields.
        // 0x0A=TX CTCSS idx, 0x0B=RX CTCSS idx, 0x0C..=TX DCS, 0x0E..=RX DCS.
        let tone = |en: u8, bytes: &[(usize, u8)]| {
            let mut h = [
                0x44, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00,
            ];
            h[CH_TONE_EN] = en;
            for &(o, v) in bytes {
                h[o] = v;
            }
            decode_channel(&make_record(&h, "X"), 0).unwrap().tone
        };
        // CTCSS both, same freq (idx 0x0d = 100.0) → collapses.
        assert_eq!(tone(0x05, &[(CH_TX_CTCSS, 0x0d), (CH_RX_CTCSS, 0x0d)]), "CTCSS 100.0");
        // Split CTCSS: TX 100.0 (0x0d) / RX 123.0 (0x13).
        assert_eq!(
            tone(0x05, &[(CH_TX_CTCSS, 0x0d), (CH_RX_CTCSS, 0x13)]),
            "TX CTCSS 100.0 / RX CTCSS 123.0"
        );
        // TX CTCSS only.
        assert_eq!(tone(0x04, &[(CH_TX_CTCSS, 0x0d)]), "TX CTCSS 100.0");
        // DCS both, same raw code 23 → collapses; label shows octal (23₁₀ = 027₈).
        assert_eq!(tone(0x0A, &[(CH_TX_DCS, 23), (CH_RX_DCS, 23)]), "DCS 027");
        // Mixed: TX CTCSS 100.0 (0x04) / RX DCS raw 23 (0x02).
        assert_eq!(
            tone(0x06, &[(CH_TX_CTCSS, 0x0d), (CH_RX_DCS, 23)]),
            "TX CTCSS 100.0 / RX DCS 027"
        );
        // No tone.
        assert_eq!(tone(0x00, &[]), "—");
    }

    #[test]
    fn empty_slots_are_skipped() {
        assert!(decode_channel(&[0xFF; CH_REC_LEN], 0).is_none());
        assert!(decode_channel(&[0x00; CH_REC_LEN], 0).is_none());
    }

    #[test]
    fn decode_channels_spans_banks_with_correct_slot_indices() {
        // Analog header for a programmed slot; placed as the first record of bank 2.
        let header = [
            0x44, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1c, 0x05, 0x0d, 0x0d, 0x11, 0x00,
            0x11, 0x00,
        ];
        let mut bank2 = vec![0xFFu8; CH_PER_BANK * CH_REC_LEN];
        let rec = make_record(&header, "BANK2-CH1");
        bank2[..CH_REC_LEN].copy_from_slice(&rec);
        // Bank 2 starts at slot 256 (0-based), so the first record is index 257.
        let banks: Vec<(usize, &[u8])> = vec![(2 * CH_PER_BANK, bank2.as_slice())];
        let (channels, names) = decode_channels(&banks);
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].index, 257);
        assert_eq!(channels[0].name, "BANK2-CH1");
        assert_eq!(names.get(&256).map(String::as_str), Some("BANK2-CH1"));
    }

    #[test]
    fn contacts_resolve_referenced_indices_by_name() {
        // Contact bank block covering indices 0..=23; put "RMH WIDE" at slot 23.
        let mut block = vec![0xFFu8; 24 * CONTACT_REC_LEN];
        let rec23 = 23 * CONTACT_REC_LEN;
        // Call type 1 (Group), alert 0, DMR ID 700 (BCD BE), name at 0x06.
        block[rec23] = 0x01;
        block[rec23 + 1] = 0x00;
        block[rec23 + 2..rec23 + 6].copy_from_slice(&[0x00, 0x00, 0x07, 0x00]);
        for (i, c) in "RMH WIDE".encode_utf16().enumerate() {
            let o = rec23 + CONTACT_NAME + i * 2;
            block[o..o + 2].copy_from_slice(&c.to_le_bytes());
        }
        let ch = AnytoneDecodedChannel {
            index: 119,
            name: "RMH700-FOCOBUCK".into(),
            rx_mhz: 445.2,
            tx_mhz: 440.2,
            shift: "-5.000".into(),
            mode: "DMR".into(),
            power: "Turbo".into(),
            bandwidth: "12.5 kHz".into(),
            tone: "—".into(),
            tone_tx: None,
            tone_rx: None,
            color_code: Some(7),
            time_slot: Some(1),
            contact_index: Some(23),
            contact_name: None,
        };
        let contacts = decode_contacts(&block, &[ch]);
        let c = contacts.get(&23).expect("referenced contact decoded");
        assert_eq!(c.name, "RMH WIDE");
        assert_eq!(c.dmr_id, 700);
        assert_eq!(c.call_type, 1); // Group
        // An index nothing references is not in the map.
        assert!(contacts.get(&5).is_none());
    }

    #[test]
    fn zone_name_decodes_utf16_and_stops_at_terminator() {
        // 0x40-byte zone-name region: "DENVER" in UTF-16LE, NUL-terminated, padded.
        let mut buf = vec![0u8; ZONE_NAME_STEP as usize];
        for (i, c) in "DENVER".encode_utf16().enumerate() {
            buf[i * 2..i * 2 + 2].copy_from_slice(&c.to_le_bytes());
        }
        assert_eq!(decode_utf16_name(&buf, ZONE_NAME_CHARS), "DENVER");
        // An all-0xFF (unprogrammed) name region decodes empty → ends the scan.
        assert_eq!(
            decode_utf16_name(&[0xFF; ZONE_NAME_STEP as usize], ZONE_NAME_CHARS),
            ""
        );
    }

    #[test]
    fn zone_resolves_member_names_and_is_terminated() {
        let mut names = HashMap::new();
        names.insert(1usize, "FOCOBUCK".to_string());
        names.insert(118usize, "AKRON".to_string());
        // indices: 0x0001, 0x0076(=118), 0x0005(unknown), 0xFFFF (stop), trailing junk.
        let data = [
            0x01, 0x00, 0x76, 0x00, 0x05, 0x00, 0xFF, 0xFF, 0x99, 0x00,
        ];
        let zone = decode_zone(&data, &names, "MY ZONE".to_string(), 1);
        assert_eq!(zone.index, 1);
        assert_eq!(zone.name, "MY ZONE");
        assert_eq!(zone.channels, vec!["FOCOBUCK", "AKRON", "#6"]);
        assert_eq!(zone.member_slots, vec![1, 118, 5]); // raw 0-based slots kept
    }

    // --- Stage 3 encode -------------------------------------------------

    #[test]
    fn ctcss_index_is_the_inverse_of_the_label() {
        // Confirmed anchors from decode: 100.0=0x0d, 110.9=0x10, 123.0=0x13.
        assert_eq!(ctcss_index(100.0), Some(0x0d));
        assert_eq!(ctcss_index(110.9), Some(0x10));
        assert_eq!(ctcss_index(123.0), Some(0x13));
        // The 150.0/151.4 pair the radio table keeps distinct.
        assert_eq!(ctcss_index(150.0), Some(25));
        assert_eq!(ctcss_index(151.4), Some(26));
        // Off-table tone rejected.
        assert_eq!(ctcss_index(101.0), None);
    }

    #[test]
    fn encode_shift_derives_direction_and_offset() {
        assert_eq!(encode_shift(446.0, 446.0), (0, [0, 0, 0, 0])); // simplex
        // -5.000 shift → group 2 (mode bit 7), 5.000 MHz offset field.
        assert_eq!(encode_shift(445.2, 440.2), (2, [0x00, 0x50, 0x00, 0x00]));
        // +5.000 shift → group 1 (mode bit 6).
        assert_eq!(encode_shift(445.2, 450.2), (1, [0x00, 0x50, 0x00, 0x00]));
    }

    /// Decode a record, then encode the decoded channel back onto a copy, and
    /// assert the two decode identically — the encoder round-trips the decoder for
    /// every modeled field.
    fn assert_encode_round_trips(rec: &[u8], edit: &AnytoneChannelEdit) {
        let mut copy = rec.to_vec();
        encode_channel_into_record(&mut copy, edit).unwrap();
        let before = decode_channel(rec, 0);
        let after = decode_channel(&copy, 0);
        assert_eq!(before, after, "decode changed after encode round-trip");
    }

    #[test]
    fn encode_round_trips_real_dmr_channel() {
        let header = [
            0x44, 0x52, 0x00, 0x00, 0x00, 0x50, 0x00, 0x00, 0x8d, 0x00, 0x00, 0x00, 0x11, 0x00,
            0x11, 0x00, 0xcf, 0x09, 0x00, 0x00, 0x17, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff,
            0xff, 0x00, 0x00, 0x00, 0x07, 0x00,
        ];
        let rec = make_record(&header, "RMH700-FOCOBUCK");
        // Build the edit from the (confirmed) decoded values: 445.2 RX, -5 shift,
        // Turbo, narrow, DMR, CC7, TS1, contact 23.
        let edit = AnytoneChannelEdit {
            name: "RMH700-FOCOBUCK".to_string(),
            rx_mhz: 445.2,
            tx_mhz: 440.2,
            power: 3,
            wide: false,
            mode: 1,
            tone: AnytoneToneEdit::default(),
            color_code: 7,
            time_slot: 1,
            contact_index: 23,
            scan_list_index: None,
        };
        assert_encode_round_trips(&rec, &edit);
    }

    #[test]
    fn encode_round_trips_real_analog_ctcss_channel() {
        let header = [
            0x44, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1c, 0x05, 0x0d, 0x0d, 0x11, 0x00,
            0x11, 0x00,
        ];
        let rec = make_record(&header, "SCAN NC DMRGMRS");
        // 446.0 simplex, Turbo, wide, FM, CTCSS 100.0 both directions.
        let edit = AnytoneChannelEdit {
            name: "SCAN NC DMRGMRS".to_string(),
            rx_mhz: 446.0,
            tx_mhz: 446.0,
            power: 3,
            wide: true,
            mode: 0,
            tone: AnytoneToneEdit {
                tx: Some(AnytoneSubTone::Ctcss(100.0)),
                rx: Some(AnytoneSubTone::Ctcss(100.0)),
            },
            color_code: 0,
            time_slot: 0,
            contact_index: 0,
            scan_list_index: None,
        };
        assert_encode_round_trips(&rec, &edit);
    }

    #[test]
    fn encode_preserves_unmodeled_bytes() {
        // Fill a record with a recognizable pattern, then encode a DMR edit and
        // confirm every byte OUTSIDE a modeled field is untouched (RMW).
        let mut rec = vec![0u8; CH_REC_LEN];
        for (i, b) in rec.iter_mut().enumerate() {
            *b = (0x33 ^ i) as u8;
        }
        let before = rec.clone();
        let edit = AnytoneChannelEdit {
            name: "X".to_string(),
            rx_mhz: 446.0,
            tx_mhz: 446.0,
            power: 2,
            wide: false,
            mode: 1, // DMR → tone bytes NOT touched
            tone: AnytoneToneEdit::default(),
            color_code: 5,
            time_slot: 2,
            contact_index: 9,
            scan_list_index: None, // None must preserve the existing 0x1B byte
        };
        encode_channel_into_record(&mut rec, &edit).unwrap();

        // The set of byte offsets the DMR encoder is allowed to write.
        let mut modeled = vec![false; CH_REC_LEN];
        for off in CH_RX..CH_RX + 4 {
            modeled[off] = true;
        }
        for off in CH_TX_OFFSET..CH_TX_OFFSET + 4 {
            modeled[off] = true;
        }
        modeled[CH_MODE] = true;
        modeled[CH_COLOR_CODE] = true;
        modeled[CH_TX_COLOR_CODE] = true;
        modeled[CH_TIME_SLOT] = true;
        modeled[CH_CONTACT] = true;
        modeled[CH_CONTACT + 1] = true;
        for off in CH_NAME..CH_NAME + CH_NAME_CHARS * 2 {
            modeled[off] = true;
        }
        for (off, m) in modeled.iter().enumerate() {
            if !m {
                assert_eq!(
                    rec[off], before[off],
                    "unmodeled byte 0x{off:02X} changed"
                );
            }
        }
        // And the modeled DMR fields did land.
        assert_eq!(rec[CH_COLOR_CODE], 5);
        assert_eq!(rec[CH_TX_COLOR_CODE], 5); // TX CC kept equal to RX CC
        assert_eq!(rec[CH_TIME_SLOT], 1); // 1-based 2 → stored 1
        assert_eq!(u16::from_le_bytes([rec[CH_CONTACT], rec[CH_CONTACT + 1]]), 9);
    }

    #[test]
    fn encode_scan_list_index_assigns_clears_and_preserves() {
        let mut rec = vec![0u8; CH_REC_LEN];
        rec[CH_SCAN_LIST] = 0x2A; // pre-existing assignment
        let base = AnytoneChannelEdit {
            name: "X".to_string(),
            rx_mhz: 446.0,
            tx_mhz: 446.0,
            power: 2,
            wide: false,
            mode: 0,
            tone: AnytoneToneEdit::default(),
            color_code: 0,
            time_slot: 0,
            contact_index: 0,
            scan_list_index: None,
        };
        // None preserves the byte.
        encode_channel_into_record(&mut rec, &base).unwrap();
        assert_eq!(rec[CH_SCAN_LIST], 0x2A);
        // Some(idx) assigns.
        let assign = AnytoneChannelEdit { scan_list_index: Some(3), ..base.clone() };
        encode_channel_into_record(&mut rec, &assign).unwrap();
        assert_eq!(rec[CH_SCAN_LIST], 3);
        // Some(0xFF) clears.
        let clear = AnytoneChannelEdit { scan_list_index: Some(0xFF), ..base };
        encode_channel_into_record(&mut rec, &clear).unwrap();
        assert_eq!(rec[CH_SCAN_LIST], 0xFF);
    }

    #[test]
    fn scan_list_record_layout_and_rejections() {
        // Defaults (unset priorities) still emit the CPS baseline bytes.
        let def = encode_scan_list_record("NOCO SCAN", &[0, 118, 0x0120], &Default::default()).unwrap();
        assert_eq!(def.len(), SCAN_LIST_STEP as usize);
        assert_eq!(def[0x00], 0); // unused
        assert_eq!(def[0x01], 0); // priority off
        assert_eq!(&def[0x02..0x06], &[0xFF; 4]); // priority indices: none
        assert_eq!(u16::from_le_bytes([def[0x06], def[0x07]]), 20); // look-back A
        assert_eq!(u16::from_le_bytes([def[0x08], def[0x09]]), 30); // look-back B
        assert_eq!(u16::from_le_bytes([def[0x0A], def[0x0B]]), 31); // dropout
        assert_eq!(u16::from_le_bytes([def[0x0C], def[0x0D]]), 31); // dwell
        assert_eq!(decode_utf16_name(&def[SCAN_NAME..], SCAN_NAME_CHARS), "NOCO SCAN");
        let members: Vec<u16> = def[SCAN_MEMBERS..SCAN_MEMBERS + SCAN_MEMBER_SLOTS * 2]
            .chunks_exact(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .collect();
        assert_eq!(&members[..3], &[0, 118, 0x0120]);
        assert!(members[3..].iter().all(|&m| m == 0xFFFF)); // all 100 slots FFFF-filled
        assert_eq!(SCAN_REVERT, 0xF8); // HW-pinned s53 (AnyTone-CPS-oracle diff)
        assert_eq!(def[SCAN_REVERT], 0); // revert = Selected Channel
        assert!(def[SCAN_REVERT + 1..].iter().all(|&b| b == 0));

        // Custom settings drive every offset. Priority ch1 = slot 5 (index 6),
        // ch2 unset (0xFFFF); Ch1+Ch2 select=3; revert=4 (Last Called).
        let s = ScanListRecordSettings {
            priority_select: 3,
            priority_slot_1: Some(5),
            priority_slot_2: None,
            look_back_a: 12,
            look_back_b: 34,
            dropout_delay: 56,
            dwell_time: 78,
            revert_channel: 4,
        };
        let rec = encode_scan_list_record("X", &[1], &s).unwrap();
        assert_eq!(rec[0x01], 3); // priority select
        assert_eq!(u16::from_le_bytes([rec[0x02], rec[0x03]]), 6); // slot 5 => index 6
        assert_eq!(u16::from_le_bytes([rec[0x04], rec[0x05]]), 0xFFFF); // ch2 none
        assert_eq!(u16::from_le_bytes([rec[0x06], rec[0x07]]), 12);
        assert_eq!(u16::from_le_bytes([rec[0x08], rec[0x09]]), 34);
        assert_eq!(u16::from_le_bytes([rec[0x0A], rec[0x0B]]), 56);
        assert_eq!(u16::from_le_bytes([rec[0x0C], rec[0x0D]]), 78);
        assert_eq!(rec[SCAN_REVERT], 4); // Last Called = 4, HW-confirmed s53

        let d = ScanListRecordSettings::default;
        assert!(encode_scan_list_record("", &[1], &d()).is_err()); // empty name
        assert!(encode_scan_list_record("X", &vec![0u16; SCAN_MAX_CHANNELS + 1], &d()).is_err());
        assert!(encode_scan_list_record("X", &[4096], &d()).is_err()); // channel out of range
        // Enum + priority-slot range rejections.
        assert!(encode_scan_list_record("X", &[1], &ScanListRecordSettings { priority_select: 4, ..d() }).is_err());
        assert!(encode_scan_list_record("X", &[1], &ScanListRecordSettings { revert_channel: 8, ..d() }).is_err());
        assert!(encode_scan_list_record("X", &[1], &ScanListRecordSettings { priority_slot_1: Some(4096), ..d() }).is_err());
    }

    #[test]
    fn contact_record_matches_patch_encoding() {
        // Same contact as `contact_patches_match_hw_capture`, as one full record.
        let rec = encode_contact_record(1, 100, "100 WYO LOCAL").unwrap();
        assert_eq!(rec.len(), CONTACT_REC_LEN);
        assert_eq!(rec[CONTACT_CALL_TYPE], 0x01);
        assert_eq!(rec[0x01], 0x00); // call alert None
        assert_eq!(&rec[CONTACT_DMR_ID..CONTACT_DMR_ID + 4], &[0x00, 0x00, 0x01, 0x00]);
        assert_eq!(decode_utf16_name(&rec[CONTACT_NAME..], CONTACT_NAME_CHARS), "100 WYO LOCAL");
        // Zero-filled tail per the descriptor.
        assert!(rec[CONTACT_NAME + CONTACT_NAME_CHARS * 2..].iter().all(|&b| b == 0));

        assert!(encode_contact_record(3, 100, "X").is_err());
        assert!(encode_contact_record(1, 100_000_000, "X").is_err());
        assert!(encode_contact_record(1, 100, "  ").is_err());
    }

    #[test]
    fn new_channel_template_carries_capture_anchored_defaults() {
        let rec = new_channel_record_template();
        assert_eq!(rec.len(), CH_REC_LEN);
        // DCS fields carry the CPS default 0x0011 (both real captures show it).
        assert_eq!(&rec[CH_TX_DCS..CH_TX_DCS + 2], &[0x11, 0x00]);
        assert_eq!(&rec[CH_RX_DCS..CH_RX_DCS + 2], &[0x11, 0x00]);
        // Custom CTCSS default 251.1 Hz (0x09CF LE), as captured.
        assert_eq!(&rec[0x10..0x12], &[0xCF, 0x09]);
        // No scan list / no RX group list.
        assert_eq!(rec[CH_SCAN_LIST], 0xFF);
        assert_eq!(rec[0x1C], 0xFF);
        // Not mistaken for an empty slot once a channel is encoded onto it.
        let mut written = rec.clone();
        let edit = AnytoneChannelEdit {
            name: "NEW".to_string(),
            rx_mhz: 446.0,
            tx_mhz: 446.0,
            power: 2,
            wide: false,
            mode: 1,
            tone: AnytoneToneEdit::default(),
            color_code: 1,
            time_slot: 1,
            contact_index: 0,
            scan_list_index: Some(0xFF),
        };
        encode_channel_into_record(&mut written, &edit).unwrap();
        assert!(!is_empty_record(&written));
        let ch = decode_channel(&written, 0).expect("decodes");
        assert_eq!(ch.name, "NEW");
        assert_eq!(ch.mode, "DMR");
    }

    #[test]
    fn encode_rejects_wrong_length_and_unknown_ctcss() {
        let mut short = vec![0u8; 10];
        let edit = AnytoneChannelEdit {
            name: "X".to_string(),
            rx_mhz: 446.0,
            tx_mhz: 446.0,
            power: 0,
            wide: false,
            mode: 0,
            tone: AnytoneToneEdit::default(),
            color_code: 0,
            time_slot: 0,
            contact_index: 0,
            scan_list_index: None,
        };
        assert!(encode_channel_into_record(&mut short, &edit).is_err());

        let mut rec = vec![0u8; CH_REC_LEN];
        let bad_tone = AnytoneChannelEdit {
            tone: AnytoneToneEdit {
                tx: Some(AnytoneSubTone::Ctcss(101.0)), // not in the table
                rx: None,
            },
            ..edit
        };
        assert!(encode_channel_into_record(&mut rec, &bad_tone).is_err());
    }

    #[test]
    fn plan_groups_edits_by_bank_and_backup_round_trips() {
        // Two edits in bank 0 + one in bank 1 → 2 planned banks; backup file parses.
        let banks = vec![
            (CHANNEL_BASE, vec![0xAAu8; 4]),
            (CHANNEL_BASE + BANK_STEP, vec![0xBB, 0xCC]),
        ];
        let bytes = serialize_backup(&banks);
        let parsed = parse_dump(&bytes).unwrap();
        assert_eq!(parsed, banks);
    }

    #[test]
    fn zone_list_record_matches_hw_capture() {
        // Zone 1 "1 - NOCO DMRGMRS" captured live 2026-07-03 @0x02000000: 20
        // 0-based u16 LE channel indices then solid 0xFF fill to 0x200.
        let indices: Vec<u16> = vec![
            0x0001, 0x0076, 0x0077, 0x008A, 0x00A1, 0x00A2, 0x00D5, 0x00D6, 0x00D7, 0x00D8,
            0x00D9, 0x00DA, 0x0058, 0x005B, 0x005A, 0x005F, 0x0018, 0x001D, 0x001E, 0x0020,
        ];
        let rec = encode_zone_list_record(&indices).unwrap();
        assert_eq!(rec.len(), ZONE_LIST_STEP as usize);
        let mut expect = vec![0xFFu8; ZONE_LIST_STEP as usize];
        for (i, idx) in indices.iter().enumerate() {
            expect[i * 2..i * 2 + 2].copy_from_slice(&idx.to_le_bytes());
        }
        assert_eq!(rec, expect);
        // Decodes back to the same 20 entries.
        let decoded: Vec<u16> = rec
            .chunks_exact(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .take_while(|&x| x != 0xFFFF)
            .collect();
        assert_eq!(decoded, indices);

        assert!(encode_zone_list_record(&vec![0u16; ZONE_MAX_CHANNELS + 1]).is_err());
        assert!(encode_zone_list_record(&[4096]).is_err()); // beyond last slot
    }

    #[test]
    fn zone_name_field_matches_hw_capture() {
        // Zone 1's name is exactly 16 chars → fills the whole field, no NUL.
        let field = encode_utf16_field("1 - NOCO DMRGMRS", ZONE_NAME_CHARS);
        let expect: Vec<u8> = "1 - NOCO DMRGMRS"
            .encode_utf16()
            .flat_map(|c| c.to_le_bytes())
            .collect();
        assert_eq!(field, expect);
        assert_eq!(decode_utf16_name(&field, ZONE_NAME_CHARS), "1 - NOCO DMRGMRS");

        // Short names NUL-terminate and zero-fill (decodes identically to the
        // CPS's stale-garbage-after-NUL variant seen on HW).
        let field = encode_utf16_field("2 - NOCO MIXED", ZONE_NAME_CHARS);
        assert_eq!(&field[28..32], &[0, 0, 0, 0]);
        assert_eq!(decode_utf16_name(&field, ZONE_NAME_CHARS), "2 - NOCO MIXED");
    }

    #[test]
    fn zone_write_patches_hit_name_and_list_addresses() {
        let writes = vec![AnytoneZoneWrite {
            zone: 3,
            edit: AnytoneZoneEdit {
                name: Some("3 - NOCO 70".into()),
                channel_indices: Some(vec![7, 9]),
            },
        }];
        let patches = zone_write_patches(&writes).unwrap();
        assert_eq!(patches.len(), 2);
        assert_eq!(patches[0].addr, ZONE_NAME_BASE + 2 * ZONE_NAME_STEP);
        assert_eq!(patches[0].data.len(), ZONE_NAME_CHARS * 2);
        assert_eq!(patches[1].addr, ZONE_LIST_BASE + 2 * ZONE_LIST_STEP);
        assert_eq!(patches[1].data.len(), ZONE_LIST_STEP as usize);

        // Rejections: zone 0, out-of-range zone, empty edit, empty name.
        let bad = |z, e: AnytoneZoneEdit| zone_write_patches(&[AnytoneZoneWrite { zone: z, edit: e }]);
        assert!(bad(0, AnytoneZoneEdit { name: Some("x".into()), channel_indices: None }).is_err());
        assert!(bad(MAX_ZONES + 1, AnytoneZoneEdit { name: Some("x".into()), channel_indices: None }).is_err());
        assert!(bad(1, AnytoneZoneEdit { name: None, channel_indices: None }).is_err());
        assert!(bad(1, AnytoneZoneEdit { name: Some("  ".into()), channel_indices: None }).is_err());
    }

    #[test]
    fn contact_patches_match_hw_capture() {
        // Contact 1 captured live 2026-07-03 @0x03A000C8: Group(01), alert 00,
        // DMR ID 100 = BCD 00 00 01 00, name "100 WYO LOCAL".
        let writes = vec![AnytoneContactWrite {
            index: 1,
            edit: AnytoneContactEdit {
                call_type: Some(1),
                call_alert: Some(0),
                dmr_id: Some(100),
                name: Some("100 WYO LOCAL".into()),
            },
        }];
        let patches = contact_write_patches(&writes).unwrap();
        let base = CONTACT_BASE + CONTACT_REC_LEN as u32;
        assert_eq!(patches[0], RegionPatch { addr: base, data: vec![0x01] });
        assert_eq!(patches[1], RegionPatch { addr: base + 1, data: vec![0x00] });
        assert_eq!(
            patches[2],
            RegionPatch { addr: base + 2, data: vec![0x00, 0x00, 0x01, 0x00] }
        );
        assert_eq!(patches[3].addr, base + CONTACT_NAME as u32);
        assert_eq!(
            decode_utf16_name(&patches[3].data, CONTACT_NAME_CHARS),
            "100 WYO LOCAL"
        );

        // Rejections: out-of-range index, bad call type, 9-digit ID, empty edit.
        let one = |idx, e: AnytoneContactEdit| {
            contact_write_patches(&[AnytoneContactWrite { index: idx, edit: e }])
        };
        assert!(one(MAX_CONTACTS_READ, AnytoneContactEdit { call_type: Some(1), call_alert: None, dmr_id: None, name: None }).is_err());
        assert!(one(0, AnytoneContactEdit { call_type: Some(3), call_alert: None, dmr_id: None, name: None }).is_err());
        assert!(one(0, AnytoneContactEdit { call_type: None, call_alert: None, dmr_id: Some(100_000_000), name: None }).is_err());
        assert!(one(0, AnytoneContactEdit { call_type: None, call_alert: None, dmr_id: None, name: None }).is_err());
    }

    #[test]
    fn window_split_groups_and_spans_boundaries() {
        // Contact record 81 (0xC8 each) straddles the 0x4000 boundary:
        // starts at 81*0xC8 = 0x3F48, ends 0x4010 → segments in windows 0 and 1.
        let rec81 = CONTACT_BASE + 81 * CONTACT_REC_LEN as u32;
        let patches = vec![
            RegionPatch { addr: ZONE_LIST_BASE, data: vec![1, 2, 3] },
            RegionPatch { addr: ZONE_LIST_BASE + 0x10, data: vec![9] },
            RegionPatch { addr: rec81, data: vec![0xAB; CONTACT_REC_LEN] },
        ];
        let by_window = split_into_windows(&patches);
        // Three windows: zone-list window (two segments), contact windows 0+1.
        assert_eq!(by_window.len(), 3);
        assert_eq!(by_window[&ZONE_LIST_BASE].len(), 2);
        let w0 = &by_window[&CONTACT_BASE];
        let w1 = &by_window[&(CONTACT_BASE + WRITE_WINDOW as u32)];
        assert_eq!(w0.len(), 1);
        assert_eq!(w1.len(), 1);
        // Segment offsets + lengths reassemble the full record.
        assert_eq!(w0[0].0, 0x3F48);
        assert_eq!(w0[0].1.len(), 0x4000 - 0x3F48);
        assert_eq!(w1[0].0, 0);
        assert_eq!(w1[0].1.len(), CONTACT_REC_LEN - (0x4000 - 0x3F48));
        // Splicing both segments back yields the original patch data.
        let mut joined = w0[0].1.clone();
        joined.extend_from_slice(&w1[0].1);
        assert_eq!(joined, vec![0xAB; CONTACT_REC_LEN]);
    }
}
