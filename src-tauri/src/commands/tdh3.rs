//! Direct TIDRADIO TD-H3 programming over the radio's built-in USB-C port.
//!
//! The TD-H3's USB-C connector enumerates as a USB serial port and speaks the
//! TIDRADIO clone protocol implemented by CHIRP's `chirp/drivers/tdh8.py`
//! (shared TD-H8/H3 driver, H3 branch). This module is a faithful Rust port so
//! 73plug can talk to the radio without going through CHIRP.
//!
//! **Phase A (this file): read-only.** Radio identify, a full-image download
//! saved as a CHIRP-compatible `.img` backup, and a decoded sanity sample.
//! There is intentionally NO write/upload path yet — writing the EEPROM can
//! brick the radio, so the read path is proven against real hardware first
//! (see PROJECT_STATE / the plan). Port enumeration reuses
//! `program::list_serial_ports` (it is model-agnostic).
//!
//! Protocol notes (ported from `tdh8.py`):
//!   - serial: 38400 8N1, 1s timeout.
//!   - identify: send the 7-byte magic `50 56 4F 4A 48 5C 14` in one write;
//!     expect ack 0x06; send 0x02; read up to 8 bytes (terminated by 0xDD);
//!     send ack 0x06 and expect 0x06 back.
//!   - read block: send `R` + u16 addr (big-endian) + u8 size; reply is a 4-byte
//!     header `W` + addr + size, then `size` data bytes, then ONE checksum byte
//!     (sum of the data, & 0xFF). No per-block ack on read.
//!   - the downloaded buffer begins with the 8-byte ident, so it is
//!     CHIRP-`.img`-compatible: radio address 0x0000 sits at BUFFER offset
//!     0x0008. Channels[200] (16 bytes each) live at buffer 0x0008, names[200]
//!     (8 chars) at buffer 0x0D48, the per-channel used/scan bitmaps at 0x1908
//!     and 0x1928. `_memsize` (0x1FEF) counts the ident prefix region.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use serialport::{ClearBuffer, SerialPort};
use tauri::{AppHandle, Manager, State};

use crate::commands::export;
use crate::commands::export::SlotChannel;
use crate::commands::program::RadioSettingsRead;
use crate::db::AppState;
use crate::error::MapErrString;
use crate::models::{Channel, RadioProfile};

const BAUD: u32 = 38400;
const TIMEOUT: Duration = Duration::from_secs(1);

/// TD-H3 identify magic (`tdh8.py` `TD_H3 = b'PVOJH\x5c\x14'`).
const MAGIC: &[u8] = &[0x50, 0x56, 0x4F, 0x4A, 0x48, 0x5C, 0x14];

/// `_memsize` for the H3: blocks are read 0x20 at a time from 0 up to this,
/// so the last block reads slightly past it to a 0x2000 radio span.
const MEMSIZE: u16 = 0x1FEF;
const BLOCK: u16 = 0x20;

// Buffer-offset memory map (image includes the 8-byte ident prefix, so a buffer
// offset = radio address + 8 — these are the CHIRP `#seekto` image offsets).
const CHANNEL_BASE: usize = 0x0008;
const NAME_BASE: usize = 0x0D48;
// Per-channel `lbit` bitmaps (LSB-first: bit k at base + k/8, mask 1 << (k%8)).
// usedflags = "channel exists" (radio's authoritative flag); scanadd = include
// in scan. Indexed by channel number - 1 (the same N-1 offset as names).
const USEDFLAGS_BASE: usize = 0x1908;
const SCANADD_BASE: usize = 0x1928;
const ENTRY_LEN: usize = 16;
const NAME_LEN: usize = 8;
/// Array length of `memory[200]`; iterated as channel numbers 1..=199
/// (`memory_bounds = (1, 199)`, `memory[0]` unused).
const CHANNEL_COUNT: usize = 200;
/// Highest programmable channel number (CHIRP `memory_bounds` upper bound).
const MAX_CHANNEL: usize = 199;
/// Radio-address range written on upload (CHIRP `_ranges_main`); blocks step
/// 0x20, the last reading to a 0x2000 span.
const UPLOAD_END: u16 = 0x1FEF;

/// Label map for the H3's 2-bit `lowpower` field. The H3 has THREE power levels
/// (CHIRP `tdh8.py` `_tx_power`): index 0 = Low (1W), 1 = Mid (4W), 2 = High
/// (8W). Uses the app's "Med" spelling so a decoded power round-trips.
const POWER_LEVELS: [&str; 3] = ["Low", "Med", "High"];

// Phase C: non-channel settings. Buffer offsets (image includes the 8-byte
// ident prefix), taken from CHIRP `tdh8.py` `MEM_FORMAT_H3`'s `settings` struct
// `#seekto 0x0CA0` and `poweron_msg` `#seekto 0x1c08`. Hardware-verified against
// real backup images. CHIRP bitfields are MSB-first within each u8 (the
// first-declared field takes the high bits), so e.g. `scanmode:2` at the top of
// its byte sits at bit positions 7-6 (lsb 6, width 2).
const SETTINGS_BASE: usize = 0x0CA0;
const POWERON_MSG_BASE: usize = 0x1C08;
const MSG_LEN: usize = 16;

// ============================================================
// Tauri commands
// ============================================================

#[derive(Serialize)]
pub struct Tdh3Ident {
    /// The raw ident bytes the radio returned, as hex.
    pub ident_hex: String,
    /// The same ident rendered as ASCII (printable bytes), e.g. "P31183" — the
    /// HAM/GMRS variants report different trailing digits.
    pub ident_ascii: String,
}

/// Harmless handshake: confirm a TD-H3 is connected and in clone mode. Reads no
/// memory, so it cannot affect the radio's contents.
#[tauri::command]
pub async fn identify_tdh3(port: String) -> Result<Tdh3Ident, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut p = open_port(&port)?;
        let ident = do_ident(&mut *p)?;
        Ok(Tdh3Ident {
            ident_hex: hex(&ident),
            ident_ascii: ascii(&ident),
        })
    })
    .await
    .estr()?
}

#[derive(Serialize, PartialEq, Debug)]
pub struct Tdh3DecodedChannel {
    pub index: usize,
    pub name: String,
    pub rx_mhz: f64,
    /// The TX shift as the radio will apply it: "" (simplex, shift bit clear),
    /// "−0.600" / "+5.000" MHz when the shift-enable bit is set, or "RX-only".
    /// Decoded from the shift-enable bit + TX/RX freq so a wrong/missing offset
    /// is visible (this is what determines whether a repeater keys up).
    pub shift: String,
    /// Human-readable tone summary, e.g. "Tone 88.5", "DTCS 023 N", or "—".
    pub tone: String,
    /// "High" or "Low" (the radio's stored TX power level).
    pub power: String,
    /// "FM" (wide) or "NFM" (narrow).
    pub mode: String,
}

#[derive(Serialize)]
pub struct Tdh3DownloadResult {
    pub ident_hex: String,
    pub ident_ascii: String,
    pub image_bytes: usize,
    /// Absolute path of the saved CHIRP-compatible backup `.img`.
    pub backup_path: String,
    /// Number of programmed (non-empty) channels found in the image.
    pub channel_count: usize,
    /// A sanity sample of the first programmed channels so the user can eyeball
    /// that the read is real before we ever build the write path.
    pub channels: Vec<Tdh3DecodedChannel>,
}

/// Read the full radio image and save it as a timestamped backup. This is the
/// non-destructive proof that the protocol port is correct, and it produces the
/// safety backup the (future) write path will always take first.
#[tauri::command]
pub async fn download_tdh3_image(
    app: AppHandle,
    port: String,
) -> Result<Tdh3DownloadResult, String> {
    let backup_dir = app.path().app_data_dir().estr()?.join("radio-backups");
    std::fs::create_dir_all(&backup_dir).estr()?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup_path = backup_dir.join(format!("tdh3-{stamp}.img"));

    tauri::async_runtime::spawn_blocking(move || {
        let mut p = open_port(&port)?;
        let ident = do_ident(&mut *p)?;
        let image = download(&mut *p, &ident)?;

        std::fs::write(&backup_path, &image)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        let channels = decode_channels(&image);
        Ok(Tdh3DownloadResult {
            ident_hex: hex(&ident),
            ident_ascii: ascii(&ident),
            image_bytes: image.len(),
            backup_path: backup_path.to_string_lossy().to_string(),
            channel_count: channels.len(),
            channels: channels.into_iter().take(20).collect(),
        })
    })
    .await
    .estr()?
}

// ============================================================
// Serial protocol
// ============================================================

fn open_port(port: &str) -> Result<Box<dyn SerialPort>, String> {
    serialport::new(port, BAUD)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        .timeout(TIMEOUT)
        .open()
        .map_err(|e| format!("could not open {port}: {e}"))
}

/// Read a single byte, or `None` on timeout (no data).
fn read_byte(p: &mut dyn SerialPort) -> Result<Option<u8>, String> {
    let mut b = [0u8; 1];
    match std::io::Read::read(p, &mut b) {
        Ok(0) => Ok(None),
        Ok(_) => Ok(Some(b[0])),
        Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Read exactly `n` bytes, erroring if the radio goes quiet mid-block.
fn read_exact(p: &mut dyn SerialPort, n: usize) -> Result<Vec<u8>, String> {
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

/// TIDRADIO clone-mode handshake. Returns the ident bytes the radio reports.
fn do_ident(p: &mut dyn SerialPort) -> Result<Vec<u8>, String> {
    let _ = p.clear(ClearBuffer::All);
    p.write_all(MAGIC).estr()?;
    if read_byte(p)? != Some(0x06) {
        return Err("no response to identify (radio off, wrong cable/port, or not a TD-H3?)".into());
    }
    p.write_all(&[0x02]).estr()?;

    // Read up to 8 bytes; the radio terminates the ident with 0xDD on some
    // firmwares (CHIRP loops range(1,9) breaking on 0xDD).
    let mut response = Vec::new();
    for _ in 0..8 {
        match read_byte(p)? {
            Some(b) => {
                response.push(b);
                if b == 0xDD {
                    break;
                }
            }
            None => break,
        }
    }
    // CHIRP accepts ident lengths of 8 or 12; a 12-byte form is compressed to
    // the canonical bytes [0, 3, 5, 7..]. We only ever see 8 here (range is 8),
    // but keep the compression for parity if a firmware returns the long form.
    let ident = if response.len() == 12 {
        let mut v = vec![response[0], response[3], response[5]];
        v.extend_from_slice(&response[7..]);
        v
    } else if response.len() == 8 {
        response
    } else {
        return Err(format!(
            "unexpected ident ({} bytes): {}",
            response.len(),
            hex(&response)
        ));
    };

    p.write_all(&[0x06]).estr()?;
    if read_byte(p)? != Some(0x06) {
        return Err("radio refused clone (no second ack)".into());
    }
    Ok(ident)
}

/// Read one memory block: `R` + addr(BE) + size, then header `W`+addr+size,
/// `size` data bytes, and a trailing checksum byte (verified).
fn read_block(p: &mut dyn SerialPort, start: u16, size: u8) -> Result<Vec<u8>, String> {
    let msg = [b'R', (start >> 8) as u8, (start & 0xFF) as u8, size];
    p.write_all(&msg).estr()?;

    let header = read_exact(p, 4)?;
    let cmd = header[0];
    let addr = u16::from_be_bytes([header[1], header[2]]);
    let length = header[3];
    if cmd != b'W' || addr != start || length != size {
        return Err(format!(
            "bad header for block 0x{start:04x}: cmd={cmd:#04x} addr={addr:#06x} len={length:#04x}"
        ));
    }

    let chunk = read_exact(p, size as usize)?;
    let checksum = read_exact(p, 1)?[0];
    let want = chunk.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    if checksum != want {
        return Err(format!(
            "checksum mismatch on block 0x{start:04x}: got {checksum:#04x}, computed {want:#04x}"
        ));
    }
    Ok(chunk)
}

/// Faithful port of `_do_download`: ident prefix, then 0x20-byte blocks from
/// radio 0x0000 up to `_memsize` (the final block reads to a 0x2000 span).
fn download(p: &mut dyn SerialPort, ident: &[u8]) -> Result<Vec<u8>, String> {
    let mut data = ident.to_vec();
    let mut addr = 0u16;
    while addr < MEMSIZE {
        data.extend(read_block(p, addr, BLOCK as u8)?);
        addr += BLOCK;
    }
    Ok(data)
}

// ============================================================
// Decoding (read-only sanity sample)
// ============================================================

/// Decode programmed channels from an image for the sanity display. Empty slots
/// (rxfreq[0] == 0xFF, CHIRP's `get_raw()[0] == 0xff`) are skipped.
///
/// Channel numbers are 1..=199 (CHIRP `memory_bounds = (1, 199)`); `memory[0]`
/// is unused. The driver stores channel N at `memory[N]` but its NAME (and
/// scan/used flags) at `names[N-1]` — a deliberate one-slot offset, so the name
/// for slot `n` is read from name index `n - 1`.
fn decode_channels(image: &[u8]) -> Vec<Tdh3DecodedChannel> {
    let mut out = Vec::new();
    for n in 1..CHANNEL_COUNT {
        let off = CHANNEL_BASE + n * ENTRY_LEN;
        if off + ENTRY_LEN > image.len() {
            break;
        }
        if image[off] == 0xFF {
            continue; // unprogrammed slot
        }
        let rx_mhz = lbcd_to_u32(&image[off..off + 4]) as f64 * 10.0 / 1_000_000.0;
        let rxtone = lbcd_to_u32(&image[off + 8..off + 10]);
        let txtone = lbcd_to_u32(&image[off + 10..off + 12]);
        let tone = decode_tone_summary(rxtone, txtone);
        let flags = image[off + 14];
        let power = POWER_LEVELS
            .get(((flags >> 4) & 0x03) as usize)
            .copied()
            .unwrap_or("Low");
        // `wide` bit set means narrow (NFM) per the driver's `wide and "NFM"`.
        let mode = if (flags >> 3) & 0x01 == 1 { "NFM" } else { "FM" };
        // Shift derived from the TX-freq bytes vs RX (how the radio does it):
        // all-0xFF TX = RX-only, equal = simplex, else the signed MHz offset.
        let txb = &image[off + 4..off + 8];
        let shift = if txb.iter().all(|&b| b == 0xFF) {
            "RX-only".to_string()
        } else {
            let tx_mhz = lbcd_to_u32(txb) as f64 * 10.0 / 1_000_000.0;
            let delta = tx_mhz - rx_mhz;
            if delta.abs() < 1e-6 {
                String::new() // simplex
            } else {
                format!("{delta:+.3}")
            }
        };
        out.push(Tdh3DecodedChannel {
            index: n,
            name: decode_name(image, n - 1),
            rx_mhz,
            shift,
            tone,
            power: power.to_string(),
            mode: mode.to_string(),
        });
    }
    out
}

fn decode_name(image: &[u8], i: usize) -> String {
    let off = NAME_BASE + i * NAME_LEN;
    if off + NAME_LEN > image.len() {
        return String::new();
    }
    image[off..off + NAME_LEN]
        .iter()
        .take_while(|&&b| b != 0xFF && b != 0x00)
        .map(|&b| b as char)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// `tdh8.py` `_decode_tone`: the lbcd value is `16665` (0xFFFF) or `0` for no
/// tone, `>= 12000` DTCS reverse (code = val - 12000), `>= 8000` DTCS normal
/// (val - 8000), else a CTCSS tone of `val / 10` Hz.
fn decode_tone_field(val: u32) -> String {
    if val == 16665 || val == 0 {
        String::new()
    } else if val >= 12000 {
        format!("DTCS {:03} R", val - 12000)
    } else if val >= 8000 {
        format!("DTCS {:03} N", val - 8000)
    } else {
        format!("Tone {:.1}", val as f64 / 10.0)
    }
}

/// Combine the decoded RX/TX tone fields into one summary cell.
fn decode_tone_summary(rxtone: u32, txtone: u32) -> String {
    let tx = decode_tone_field(txtone);
    let rx = decode_tone_field(rxtone);
    match (tx.is_empty(), rx.is_empty()) {
        (true, true) => "—".to_string(),
        (false, true) => format!("TX {tx}"),
        (true, false) => format!("RX {rx}"),
        (false, false) if tx == rx => tx,
        (false, false) => format!("TX {tx} / RX {rx}"),
    }
}

/// Little-endian BCD (CHIRP `lbcd`): byte[0] is the least-significant digit
/// pair; within a byte the high nibble is tens, the low nibble is ones.
fn lbcd_to_u32(b: &[u8]) -> u32 {
    let mut v = 0u32;
    let mut mult = 1u32;
    for &byte in b {
        let lo = (byte & 0x0F) as u32;
        let hi = (byte >> 4) as u32;
        v += (hi * 10 + lo) * mult;
        mult *= 100;
    }
    v
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Render printable bytes as ASCII, non-printables as '.'.
fn ascii(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if (0x20..0x7F).contains(&b) {
                b as char
            } else {
                '.'
            }
        })
        .collect()
}

// ============================================================
// Phase B: write / upload
// ============================================================

#[derive(Serialize)]
pub struct Tdh3ProgramResult {
    /// Channels written (to channel numbers 1..=written).
    pub written: usize,
    /// Channel slots cleared (so the radio matches the codeplug exactly).
    pub cleared: usize,
    /// Whether the post-write read-back matched what we intended to write.
    pub verified: bool,
    /// Set when verification could not run or found differences.
    pub verify_note: Option<String>,
    /// Absolute path of the pre-write backup `.img`.
    pub backup_path: String,
    /// Channels present on the radio after writing (read back), sample.
    pub channels: Vec<Tdh3DecodedChannel>,
}

/// Program a codeplug's channels directly into a connected TD-H3.
///
/// Safety model (mirrors the UV-5R path): download the full image and save it as
/// a backup FIRST, patch only the channel/name regions and the used/scan bitmaps
/// into that downloaded image, upload the whole main range (so every untouched
/// byte — including all radio settings — is written back exactly as read), then
/// read the radio back to confirm. Phase B writes channels only; the radio's
/// non-channel settings are preserved as-is (Phase C will write those).
#[tauri::command]
pub async fn program_tdh3_codeplug(
    app: AppHandle,
    state: State<'_, AppState>,
    codeplug_id: i64,
    port: String,
) -> Result<Tdh3ProgramResult, String> {
    let (model, slots) = export::resolve_codeplug_slots(&state.pool, codeplug_id).await?;
    if model.model != "TD-H3" {
        return Err(format!(
            "This programmer is for the TIDRADIO TD-H3 (this codeplug targets {}).",
            model.display_name
        ));
    }
    if slots.len() > MAX_CHANNEL {
        return Err(format!(
            "Codeplug has {} programmable channels, but the TD-H3 holds only {MAX_CHANNEL}.",
            slots.len()
        ));
    }

    let codeplug_name: String = sqlx::query_scalar("SELECT name FROM codeplugs WHERE id = ?1")
        .bind(codeplug_id)
        .fetch_one(&state.pool)
        .await
        .estr()?;

    let backup_dir = app.path().app_data_dir().estr()?.join("radio-backups");
    std::fs::create_dir_all(&backup_dir).estr()?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    // Slug the codeplug name into the backup filename so multiple codeplugs for
    // the radio stay distinguishable when restoring.
    let slug = slug_label(&codeplug_name);
    let backup_path = backup_dir.join(if slug.is_empty() {
        format!("tdh3-prewrite-{stamp}.img")
    } else {
        format!("tdh3-prewrite-{slug}-{stamp}.img")
    });

    let result = tauri::async_runtime::spawn_blocking(move || -> Result<Tdh3ProgramResult, String> {
        let mut p = open_port(&port)?;

        // 1. Download + back up the current radio contents.
        let ident = do_ident(&mut *p)?;
        let mut image = download(&mut *p, &ident)?;
        std::fs::write(&backup_path, &image)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        // 2. Patch channels/names + used/scan bitmaps into the downloaded image.
        let written = slots.len();
        patch_image(&mut image, &slots);

        // 3. Re-identify (the radio settles after a full read) and upload the
        //    whole main range, then exit programming mode.
        std::thread::sleep(Duration::from_secs(1));
        reident(&mut *p)?;
        upload(&mut *p, &image)?;

        // 4. Read back and verify (non-fatal — every block was ack'd).
        let (verified, verify_note, channels) = match verify_after_write(&mut *p, &image) {
            Ok((ok, note, ch)) => (ok, note, ch),
            Err(e) => (
                false,
                Some(format!(
                    "Write completed, but read-back verification could not run ({e}). \
                     Power-cycle the radio and use Download to confirm."
                )),
                decode_channels(&image),
            ),
        };

        Ok(Tdh3ProgramResult {
            written,
            cleared: MAX_CHANNEL - written,
            verified,
            verify_note,
            backup_path: backup_path.to_string_lossy().to_string(),
            channels: channels.into_iter().take(20).collect(),
        })
    })
    .await
    .estr()??;

    // The write ack'd every block, so stamp the codeplug's last program time
    // (the Codeplugs screen shows "Programmed <date>"). Verification is
    // best-effort and doesn't gate the stamp.
    sqlx::query(
        "UPDATE codeplugs SET last_exported = CURRENT_TIMESTAMP, last_export_kind = 'radio' WHERE id = ?1",
    )
    .bind(codeplug_id)
    .execute(&state.pool)
    .await
    .estr()?;

    Ok(result)
}

/// Slug a human label to a filesystem-safe, lowercase, dash-separated form
/// (empty if the label has no alphanumerics), capped to 40 chars.
fn slug_label(label: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !slug.is_empty() {
            slug.push('-');
            prev_dash = true;
        }
    }
    slug.trim_end_matches('-').chars().take(40).collect()
}

/// Re-establish a clone session, retrying over a few seconds since the radio can
/// still be settling right after a full read.
fn reident(p: &mut dyn SerialPort) -> Result<Vec<u8>, String> {
    const ATTEMPTS: usize = 5;
    let mut last = String::from("no contact with radio");
    for attempt in 0..ATTEMPTS {
        let _ = p.clear(ClearBuffer::All);
        match do_ident(p) {
            Ok(ident) => return Ok(ident),
            Err(e) => {
                last = e;
                if attempt + 1 < ATTEMPTS {
                    std::thread::sleep(Duration::from_millis(1500));
                }
            }
        }
    }
    Err(format!(
        "radio did not re-enter clone mode after the previous step ({last}). \
         Put it back in clone mode (power-cycle) and retry."
    ))
}

/// Re-read the radio and compare decoded channels to what we intended.
fn verify_after_write(
    p: &mut dyn SerialPort,
    intended: &[u8],
) -> Result<(bool, Option<String>, Vec<Tdh3DecodedChannel>), String> {
    std::thread::sleep(Duration::from_secs(1));
    let ident = reident(p)?;
    let readback = download(p, &ident)?;

    let expected = decode_channels(intended);
    let actual = decode_channels(&readback);
    if expected == actual {
        Ok((true, None, actual))
    } else {
        let note = format!(
            "Read-back shows {} channels; {} were intended. The pre-write backup is saved if you need to revert.",
            actual.len(),
            expected.len()
        );
        Ok((false, Some(note), actual))
    }
}

/// Overwrite every channel slot (1..=MAX_CHANNEL) so the radio matches the
/// codeplug exactly: each included channel lands at `memory[N]` (name/flags at
/// the N-1 offset) and every other slot is cleared. Slot `s` (0-based, packed
/// from `resolve_codeplug_slots`) maps to channel number `N = s + 1`.
fn patch_image(image: &mut [u8], slots: &[SlotChannel]) {
    let mut assigned: Vec<Option<&SlotChannel>> = vec![None; MAX_CHANNEL + 1];
    for s in slots {
        let n = s.slot + 1;
        if n <= MAX_CHANNEL {
            assigned[n] = Some(s);
        }
    }

    for n in 1..=MAX_CHANNEL {
        let co = CHANNEL_BASE + n * ENTRY_LEN;
        let no = NAME_BASE + (n - 1) * NAME_LEN;
        let k = n - 1; // used/scan bit index
        if let Some(s) = assigned[n] {
            image[co..co + ENTRY_LEN].copy_from_slice(&encode_channel(&s.channel));
            image[no..no + NAME_LEN].copy_from_slice(&name_bytes(&s.name));
            set_flag(image, USEDFLAGS_BASE, k, true);
            set_flag(image, SCANADD_BASE, k, true);
        } else {
            // Empty: 0xFF memory + name, used/scan bits cleared (CHIRP's empty).
            image[co..co + ENTRY_LEN].fill(0xFF);
            image[no..no + NAME_LEN].fill(0xFF);
            set_flag(image, USEDFLAGS_BASE, k, false);
            set_flag(image, SCANADD_BASE, k, false);
        }
    }
}

/// Set or clear bit `k` in an `lbit` bitmap (LSB-first within each byte).
fn set_flag(image: &mut [u8], base: usize, k: usize, on: bool) {
    let byte = base + k / 8;
    let mask = 1u8 << (k % 8);
    if on {
        image[byte] |= mask;
    } else {
        image[byte] &= !mask;
    }
}

/// Encode one 16-byte TD-H3 channel block. Starts zeroed (as CHIRP's
/// `fill_raw(0x00)`) then sets the defined fields.
fn encode_channel(c: &Channel) -> [u8; ENTRY_LEN] {
    let mut m = [0u8; ENTRY_LEN];

    let tx = export::tx_frequency(c);
    m[0..4].copy_from_slice(&freq_to_lbcd(c.rx_freq));
    m[4..8].copy_from_slice(&freq_to_lbcd(tx));

    let (rxtone, txtone) = encode_tones(c);
    m[8..10].copy_from_slice(&rxtone);
    m[10..12].copy_from_slice(&txtone);

    // byte 12 (unused1) and byte 13 (pttid/freqhop/bcl) stay 0.
    // byte 14: lowpower bits 5-4, wide bit 3, offset bits 1-0 (left 0). The
    // radio derives the TX shift +/-/none from the stored TX vs RX frequencies
    // — verified against the factory image, where -0.6 / +5 repeater channels
    // carry byte14 = 0x10, identical to simplex (only the TX-freq bytes differ).
    // So NOTHING else encodes the shift; the TX-freq bytes above are sufficient.
    let power = power_index(c);
    let wide: u8 = if c.mode.as_deref().map(|s| s.eq_ignore_ascii_case("FM")) == Some(true) {
        0
    } else {
        1
    };
    m[14] = (power << 4) | (wide << 3);
    // byte 15 (unused10) stays 0.

    m
}

/// 8-byte name cell: ASCII preserved (the TD-H3 charset includes mixed case),
/// unused cells padded with 0x00 (CHIRP writes `"\x00"` past the name).
fn name_bytes(name: &str) -> [u8; NAME_LEN] {
    let mut out = [0u8; NAME_LEN];
    for (i, ch) in name.chars().take(NAME_LEN).enumerate() {
        out[i] = if ch.is_ascii() { ch as u8 } else { 0x00 };
    }
    out
}

/// Encode RX/TX frequency (MHz) as the radio's little-endian BCD (freq/10 Hz).
fn freq_to_lbcd(mhz: f64) -> [u8; 4] {
    let mut value = (mhz * 100_000.0).round() as u32; // MHz -> units of 10 Hz
    let mut out = [0u8; 4];
    for b in out.iter_mut() {
        let lo = (value % 10) as u8;
        value /= 10;
        let hi = (value % 10) as u8;
        value /= 10;
        *b = (hi << 4) | lo;
    }
    out
}

/// `lowpower` index into the H3's THREE-level `_tx_power`
/// = [Low 1W (0), Mid 4W (1), High 8W (2)] (CHIRP `tdh8.py`). The app's
/// radio-agnostic levels are High / Med / Low: High → 2, Med → 1, Low → 0.
/// Unset → 2 (radio max), matching CHIRP's `index(mem.power or _tx_power[-1])`.
/// NOTE: this previously mapped "High"/unset → 1, which is *Mid* (4 W), so every
/// "High" channel transmitted at half power — fixed to 2 so High = the 8 W max.
fn power_index(c: &Channel) -> u8 {
    match c.power.as_deref() {
        Some(p) if p.eq_ignore_ascii_case("Low") => 0,
        Some(p)
            if p.eq_ignore_ascii_case("Med")
                || p.eq_ignore_ascii_case("Medium")
                || p.eq_ignore_ascii_case("Mid") =>
        {
            1
        }
        _ => 2,
    }
}

/// Build a 2-byte little-endian BCD value (CHIRP `lbcd[2]`): byte0 = the low two
/// decimal digits, byte1 = the next two.
fn bcd2(v: u32) -> [u8; 2] {
    let lo = (v % 100) as u8;
    let hi = ((v / 100) % 100) as u8;
    [
        ((lo / 10) << 4) | (lo % 10),
        ((hi / 10) << 4) | (hi % 10),
    ]
}

/// Encode one tone field (rxtone or txtone) — CHIRP `_encode_tone`:
///   off  -> 0xFF 0xFF
///   Tone -> BCD of freq*10 (e.g. 88.5 -> 0885)
///   DTCS -> BCD of the code, with byte[1] OR 0x80 (normal) / 0xC0 (reverse).
fn tone_off() -> [u8; 2] {
    [0xFF, 0xFF]
}
fn tone_ctcss(hz: f64) -> [u8; 2] {
    bcd2((hz * 10.0).round() as u32)
}
fn tone_dtcs(code: &Option<String>, reverse: bool) -> [u8; 2] {
    match code.as_deref().and_then(|s| s.trim().parse::<u32>().ok()) {
        Some(n) => {
            let mut b = bcd2(n);
            b[1] |= if reverse { 0xC0 } else { 0x80 };
            b
        }
        None => tone_off(),
    }
}

/// (rxtone, txtone) field pair using the TD-H3 BCD tone encoding. tone_mode
/// follows the project's universal scheme (off/Tone/TSQL/DTCS/Cross); the
/// per-side split mirrors CHIRP `split_tone_encode` (see the UV-5R `encode_tones`).
fn encode_tones(c: &Channel) -> ([u8; 2], [u8; 2]) {
    let pol = c.dcs_polarity.as_bytes();
    let tx_rev = pol.first() == Some(&b'R');
    let rx_rev = pol.get(1) == Some(&b'R');
    let ctcss = |t: Option<f64>| t.map(tone_ctcss).unwrap_or_else(tone_off);

    let mode = c.tone_mode.as_deref().unwrap_or("off");
    if mode.eq_ignore_ascii_case("Tone") {
        (tone_off(), ctcss(c.ctcss_uplink))
    } else if mode.eq_ignore_ascii_case("TSQL") {
        let t = ctcss(c.ctcss_downlink);
        (t, t)
    } else if mode.eq_ignore_ascii_case("DTCS") {
        (
            tone_dtcs(&c.dcs_code, rx_rev),
            tone_dtcs(&c.dcs_code, tx_rev),
        )
    } else if mode.eq_ignore_ascii_case("Cross") {
        let (txmode, rxmode) = c.cross_mode.split_once("->").unwrap_or(("", ""));
        let tx = if txmode.eq_ignore_ascii_case("Tone") {
            ctcss(c.ctcss_uplink)
        } else if txmode.eq_ignore_ascii_case("DTCS") {
            tone_dtcs(&c.dcs_code, tx_rev)
        } else {
            tone_off()
        };
        let rx = if rxmode.eq_ignore_ascii_case("Tone") {
            ctcss(c.ctcss_downlink)
        } else if rxmode.eq_ignore_ascii_case("DTCS") {
            tone_dtcs(&c.dcs_rx_code, rx_rev)
        } else {
            tone_off()
        };
        (rx, tx)
    } else {
        (tone_off(), tone_off())
    }
}

/// Upload the whole main range in 0x20-byte `W` blocks (data + 1 checksum byte),
/// then send `E` to leave programming mode. Buffer offset = radio address + 8.
fn upload(p: &mut dyn SerialPort, image: &[u8]) -> Result<(), String> {
    let mut addr = 0u16;
    while addr < UPLOAD_END {
        let off = addr as usize + 8;
        send_block(p, addr, &image[off..off + BLOCK as usize])?;
        addr += BLOCK;
    }
    p.write_all(b"E").estr()?;
    Ok(())
}

/// Write one 0x20-byte block: `W` + addr(BE) + size + data + checksum (sum of the
/// data bytes & 0xFF); expect ack 0x06.
fn send_block(p: &mut dyn SerialPort, addr: u16, data: &[u8]) -> Result<(), String> {
    let mut msg = vec![b'W', (addr >> 8) as u8, (addr & 0xFF) as u8, BLOCK as u8];
    msg.extend_from_slice(data);
    msg.push(data.iter().fold(0u8, |a, &b| a.wrapping_add(b)));
    p.write_all(&msg).estr()?;
    match read_byte(p)? {
        Some(0x06) => Ok(()),
        _ => Err(format!("radio refused to accept block 0x{addr:04x}")),
    }
}

// ============================================================
// Phase C: non-channel settings (read / edit / write)
// ============================================================

/// The radio-global "options" 73plug exposes for editing — the common-settings
/// subset (the DTMF group and the six side/top-key assignments are intentionally
/// left out for now). Multi-value fields are carried as a zero-based index into
/// the matching label list on the frontend (see `Tdh3SETTINGS_*` lists there);
/// switches are plain booleans. These are NOT tied to a codeplug: they are read
/// from and written straight back to the connected radio.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Tdh3Settings {
    /// Squelch level 0..=9.
    pub squelch: u8,
    /// Display brightness as a 0..=4 index (label "1".."5"). Stored inverted on
    /// the radio (stored = 4 - index), handled in decode/encode.
    pub brightness: u8,
    /// Backlight time-out `ligcon`, index 0..=4 (CONT/5s/10s/15s/30s).
    pub backlight: u8,
    /// VOX gain `voxgain`, index 0..=5 (Off/1..5).
    pub vox_gain: u8,
    /// VOX delay `voxdelay`, index 0..=2 (1.05s/2.0s/3.0s).
    pub vox_delay: u8,
    /// Transmit time-out timer `tot`, index 0..=7 (Off/30S..210S).
    pub tot: u8,
    /// Scan resume mode `scanmode`, index 0..=2 (TO/CO/SE).
    pub scan_mode: u8,
    /// Power-on display `ponmsg`, index 0..=2 (Off/Msg/Icon).
    pub power_on_msg_mode: u8,
    /// Battery save `save`, index 0..=5 (Off/1:1/1:2/1:3/1:4/1:8).
    pub power_save: u8,
    /// Breathing LED `breathled`, index 0..=4 (Off/5S/10S/15S/30S).
    pub breath_led: u8,
    /// Display A mode `mdfa`, index 0..=1 (Frequency/Name).
    pub display_a: u8,
    /// Display B mode `mdfb`, index 0..=1 (Frequency/Name).
    pub display_b: u8,
    /// Alarm mode `alarm`, index 0..=1 (On site/Alarm).
    pub alarm_mode: u8,
    /// Key beep `btnvoice`.
    pub beep: bool,
    /// Voice prompt `voiceprompt`.
    pub voice_prompt: bool,
    /// Keypad auto-lock `keyautolock`.
    pub auto_lock: bool,
    /// Dual watch `dbrx`.
    pub dual_watch: bool,
    /// Enable TX on the 220 MHz band `tx220`.
    pub tx_220: bool,
    /// Enable TX on the 350 MHz band `tx350`.
    pub tx_350: bool,
    /// Enable TX on the 500 MHz band `tx500`.
    pub tx_500: bool,
    /// AM band receive `amband`.
    pub am_band: bool,
    /// Three lines of power-on message text (16 chars each).
    pub power_on_msg1: String,
    pub power_on_msg2: String,
    pub power_on_msg3: String,
}

/// Extract `width` bits at LSB position `lsb` from `byte`.
fn bits(byte: u8, lsb: u8, width: u8) -> u8 {
    let mask = ((1u16 << width) - 1) as u8;
    (byte >> lsb) & mask
}

/// Replace `width` bits at LSB position `lsb` in `byte` with `val`'s low bits.
fn set_bits(byte: &mut u8, lsb: u8, width: u8, val: u8) {
    let field = ((1u16 << width) - 1) as u8;
    let mask = field << lsb;
    *byte = (*byte & !mask) | ((val & field) << lsb);
}

/// Decode the editable settings from a full radio image. Byte indices are
/// relative to `SETTINGS_BASE`; see the struct field docs for each meaning.
fn decode_settings(image: &[u8]) -> Tdh3Settings {
    let s = &image[SETTINGS_BASE..];
    let bit = |i: usize, b: u8| (s[i] >> b) & 1 == 1;
    Tdh3Settings {
        squelch: s[17],
        brightness: 4u8.saturating_sub(s[5]),
        backlight: s[21],
        vox_gain: bits(s[15], 0, 3),
        vox_delay: s[22],
        tot: s[18],
        scan_mode: bits(s[9], 6, 2),
        power_on_msg_mode: bits(s[11], 6, 2),
        power_save: s[20],
        breath_led: bits(s[23], 4, 3),
        display_a: bits(s[10], 2, 1),
        display_b: bits(s[11], 4, 1),
        alarm_mode: bits(s[23], 0, 1),
        beep: bit(9, 2),
        voice_prompt: bit(9, 0),
        auto_lock: bit(9, 4),
        dual_watch: bit(11, 2),
        tx_220: bit(19, 4),
        tx_350: bit(19, 3),
        tx_500: bit(19, 2),
        am_band: bit(23, 1),
        power_on_msg1: decode_msg(image, 0),
        power_on_msg2: decode_msg(image, 1),
        power_on_msg3: decode_msg(image, 2),
    }
}

/// Patch the editable settings into a downloaded image in place, touching only
/// the specific bits/bytes each field owns (every other byte — including the
/// unknown/reserved bits packed alongside — is preserved exactly as read).
fn encode_settings(image: &mut [u8], st: &Tdh3Settings) {
    let b = SETTINGS_BASE;
    image[b + 17] = st.squelch.min(9);
    image[b + 5] = 4u8.saturating_sub(st.brightness.min(4));
    image[b + 21] = st.backlight.min(4);
    set_bits(&mut image[b + 15], 0, 3, st.vox_gain.min(5));
    image[b + 22] = st.vox_delay.min(2);
    image[b + 18] = st.tot.min(7);
    set_bits(&mut image[b + 9], 6, 2, st.scan_mode.min(2));
    set_bits(&mut image[b + 11], 6, 2, st.power_on_msg_mode.min(2));
    image[b + 20] = st.power_save.min(5);
    set_bits(&mut image[b + 23], 4, 3, st.breath_led.min(4));
    set_bits(&mut image[b + 10], 2, 1, st.display_a & 1);
    set_bits(&mut image[b + 11], 4, 1, st.display_b & 1);
    set_bits(&mut image[b + 23], 0, 1, st.alarm_mode & 1);
    set_bits(&mut image[b + 9], 2, 1, st.beep as u8);
    set_bits(&mut image[b + 9], 0, 1, st.voice_prompt as u8);
    set_bits(&mut image[b + 9], 4, 1, st.auto_lock as u8);
    set_bits(&mut image[b + 11], 2, 1, st.dual_watch as u8);
    set_bits(&mut image[b + 19], 4, 1, st.tx_220 as u8);
    set_bits(&mut image[b + 19], 3, 1, st.tx_350 as u8);
    set_bits(&mut image[b + 19], 2, 1, st.tx_500 as u8);
    set_bits(&mut image[b + 23], 1, 1, st.am_band as u8);
    encode_msg(image, 0, &st.power_on_msg1);
    encode_msg(image, 1, &st.power_on_msg2);
    encode_msg(image, 2, &st.power_on_msg3);
}

/// Read one 16-byte power-on message line (left-aligned, null/0xFF-padded).
/// Non-printable bytes render as a space so a junk byte stays visible.
fn decode_msg(image: &[u8], i: usize) -> String {
    let off = POWERON_MSG_BASE + i * MSG_LEN;
    if off + MSG_LEN > image.len() {
        return String::new();
    }
    image[off..off + MSG_LEN]
        .iter()
        .take_while(|&&b| b != 0x00 && b != 0xFF)
        .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { ' ' })
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Write one 16-byte power-on message line: left-aligned ASCII, null-padded,
/// non-ASCII (or out-of-range) chars folded to a space.
fn encode_msg(image: &mut [u8], i: usize, text: &str) {
    let off = POWERON_MSG_BASE + i * MSG_LEN;
    let mut buf = [0u8; MSG_LEN];
    for (j, ch) in text.chars().take(MSG_LEN).enumerate() {
        let c = ch as u32;
        buf[j] = if (0x20..0x7F).contains(&c) { c as u8 } else { b' ' };
    }
    image[off..off + MSG_LEN].copy_from_slice(&buf);
}

/// Read the radio's current settings into a profile's editor form (ident →
/// download → decode into the schema-keyed JSON the profile stores). The TD-H3
/// mirror of the UV-5R `read_radio_settings`: non-destructive (reads memory
/// only), backs up the downloaded image first, and decodes only the keys we can
/// locate so the editor can merge them over the profile's current values.
#[tauri::command]
pub async fn read_tdh3_settings_for_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    profile_id: i64,
    port: String,
) -> Result<RadioSettingsRead, String> {
    // The profile's model + settings schema decide what we can decode.
    let row: Option<(String, Option<String>, String)> = sqlx::query_as(
        "SELECT rm.model, rm.non_channel_settings_schema, rp.display_name \
         FROM radio_profiles rp JOIN radio_models rm ON rm.id = rp.radio_model_id \
         WHERE rp.id = ?1",
    )
    .bind(profile_id)
    .fetch_optional(&state.pool)
    .await
    .estr()?;
    let (model, schema, profile_name) = row.ok_or("radio profile not found")?;
    if model != "TD-H3" {
        return Err(format!(
            "This radio profile is for {model}, not the TD-H3."
        ));
    }
    let schema = schema.ok_or("the TD-H3 model has no settings schema to decode into")?;

    let backup_dir = app.path().app_data_dir().estr()?.join("radio-backups");
    std::fs::create_dir_all(&backup_dir).estr()?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let slug = slug_label(&profile_name);
    let backup_path = backup_dir.join(if slug.is_empty() {
        format!("tdh3-settings-{stamp}.img")
    } else {
        format!("tdh3-settings-{slug}-{stamp}.img")
    });

    tauri::async_runtime::spawn_blocking(move || {
        let mut p = open_port(&port)?;
        let ident = do_ident(&mut *p)?;
        let image = download(&mut *p, &ident)?;
        std::fs::write(&backup_path, &image)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        let settings = decode_profile_settings(&image, &schema)?;
        let count = settings.as_object().map(|o| o.len()).unwrap_or(0);
        Ok(RadioSettingsRead {
            settings,
            count,
            backup_path: backup_path.to_string_lossy().to_string(),
        })
    })
    .await
    .estr()?
}

#[derive(Serialize)]
pub struct Tdh3SettingsWriteResult {
    /// Whether the post-write read-back matched what we intended to write.
    pub verified: bool,
    /// Set when verification could not run or found differences.
    pub verify_note: Option<String>,
    /// Absolute path of the pre-write backup `.img`.
    pub backup_path: String,
    /// The settings present on the radio after writing (read back).
    pub settings: Tdh3Settings,
}

/// Write edited settings back to a connected TD-H3.
///
/// Same safety model as the channel write: download the full image and back it
/// up FIRST, patch only the settings bits into that image (every other byte —
/// channels included — is preserved exactly as read), upload the whole main
/// range, then read the radio back and confirm the settings took.
#[tauri::command]
pub async fn write_tdh3_settings(
    app: AppHandle,
    port: String,
    settings: Tdh3Settings,
) -> Result<Tdh3SettingsWriteResult, String> {
    let backup_dir = app.path().app_data_dir().estr()?.join("radio-backups");
    std::fs::create_dir_all(&backup_dir).estr()?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup_path = backup_dir.join(format!("tdh3-presettings-{stamp}.img"));

    tauri::async_runtime::spawn_blocking(move || {
        let mut p = open_port(&port)?;

        // 1. Download + back up the current radio contents.
        let ident = do_ident(&mut *p)?;
        let mut image = download(&mut *p, &ident)?;
        std::fs::write(&backup_path, &image)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        // 2. Patch the settings bits into the downloaded image.
        encode_settings(&mut image, &settings);

        // 3. Re-identify and upload the whole main range, then leave clone mode.
        std::thread::sleep(Duration::from_secs(1));
        reident(&mut *p)?;
        upload(&mut *p, &image)?;

        // 4. Read back and verify against what we intended (non-fatal).
        let (verified, verify_note, result) = match verify_settings_after_write(&mut *p, &image) {
            Ok((ok, note, st)) => (ok, note, st),
            Err(e) => (
                false,
                Some(format!(
                    "Write completed, but read-back verification could not run ({e}). \
                     Power-cycle the radio and use Read to confirm."
                )),
                decode_settings(&image),
            ),
        };

        Ok(Tdh3SettingsWriteResult {
            verified,
            verify_note,
            backup_path: backup_path.to_string_lossy().to_string(),
            settings: result,
        })
    })
    .await
    .estr()?
}

/// Re-read the radio and compare decoded settings to what we intended to write.
fn verify_settings_after_write(
    p: &mut dyn SerialPort,
    intended: &[u8],
) -> Result<(bool, Option<String>, Tdh3Settings), String> {
    std::thread::sleep(Duration::from_secs(1));
    let ident = reident(p)?;
    let readback = download(p, &ident)?;

    let expected = decode_settings(intended);
    let actual = decode_settings(&readback);
    if expected == actual {
        Ok((true, None, actual))
    } else {
        Ok((
            false,
            Some(
                "Read-back settings differ from what was sent. The pre-write backup is saved \
                 if you need to revert."
                    .to_string(),
            ),
            actual,
        ))
    }
}

// ============================================================
// Phase C (cont.): apply a saved radio profile's settings
// ============================================================
//
// The TD-H3 radio profile stores its `non_channel_settings` as JSON keyed by the
// CHIRP `RadioSetting` names in the model's settings schema (squelch, ligcon,
// btnvoice, ponmsg, brightness, tx220…). This mirrors the UV-5R
// `radios::baofeng_uv5r::settings::apply_profile_settings`: patch each KNOWN key's bits into a
// downloaded image (unknown/unsupported keys — the deferred DTMF + key groups —
// are skipped, so they're preserved as read), then upload. Only the settings
// region changes; channels are untouched.

/// Where one TD-H3 schema key lives, as an index relative to `SETTINGS_BASE`.
enum SLoc {
    /// Whole byte = the resolved index.
    Byte(usize),
    /// Whole byte, but stored INVERTED as `4 - index` (brightness only).
    BrightnessByte(usize),
    /// `width` bits at LSB `lsb` within the byte (others preserved).
    Bits(usize, u8, u8),
    /// One of the three 16-byte power-on message lines (0..=2).
    Msg(usize),
}

/// Resolve a TD-H3 schema key to its storage location, or `None` for keys we
/// don't write (the deferred DTMF group + side/top-key assignments).
fn tdh3_loc(key: &str) -> Option<SLoc> {
    Some(match key {
        "squelch" => SLoc::Byte(17),
        "brightness" => SLoc::BrightnessByte(5),
        "ligcon" => SLoc::Byte(21),
        "voxgain" => SLoc::Bits(15, 0, 3),
        "voxdelay" => SLoc::Byte(22),
        "tot" => SLoc::Byte(18),
        "scanmode" => SLoc::Bits(9, 6, 2),
        "ponmsg" => SLoc::Bits(11, 6, 2),
        "save" => SLoc::Byte(20),
        "breathled" => SLoc::Bits(23, 4, 3),
        "mdfa" => SLoc::Bits(10, 2, 1),
        "mdfb" => SLoc::Bits(11, 4, 1),
        "alarm" => SLoc::Bits(23, 0, 1),
        "btnvoice" => SLoc::Bits(9, 2, 1),
        "voiceprompt" => SLoc::Bits(9, 0, 1),
        "keyautolock" => SLoc::Bits(9, 4, 1),
        "dbrx" => SLoc::Bits(11, 2, 1),
        "tx220" => SLoc::Bits(19, 4, 1),
        "tx350" => SLoc::Bits(19, 3, 1),
        "tx500" => SLoc::Bits(19, 2, 1),
        "amband" => SLoc::Bits(23, 1, 1),
        "poweron_msg.msg1" => SLoc::Msg(0),
        "poweron_msg.msg2" => SLoc::Msg(1),
        "poweron_msg.msg3" => SLoc::Msg(2),
        _ => return None,
    })
}

/// Build a `key -> option labels` map from the model schema, so a stored select
/// label (e.g. "15s", "Name", "Icon") can be turned back into its byte index.
fn schema_options(schema_json: &str) -> Result<HashMap<String, Vec<String>>, String> {
    let schema: Value = serde_json::from_str(schema_json)
        .map_err(|e| format!("invalid settings schema JSON: {e}"))?;
    let arr = schema.as_array().ok_or("settings schema is not a JSON array")?;
    let mut map = HashMap::new();
    for field in arr {
        if let (Some(key), Some(opts)) = (
            field.get("key").and_then(Value::as_str),
            field.get("options").and_then(Value::as_array),
        ) {
            let labels = opts
                .iter()
                .filter_map(|o| o.as_str().map(str::to_string))
                .collect();
            map.insert(key.to_string(), labels);
        }
    }
    Ok(map)
}

/// Resolve a profile value to a byte index: booleans → 0/1, numbers as-is,
/// select labels → their option index.
fn resolve_index(key: &str, value: &Value, options: &HashMap<String, Vec<String>>) -> Option<i64> {
    match value {
        Value::Bool(b) => Some(*b as i64),
        Value::Number(n) => n.as_i64(),
        Value::String(s) => options
            .get(key)
            .and_then(|opts| opts.iter().position(|o| o == s))
            .map(|i| i as i64),
        _ => None,
    }
}

/// Patch a profile's `non_channel_settings` into a downloaded image. Returns the
/// number of fields applied (unknown keys and unresolvable values are skipped so
/// a partial/odd profile can't corrupt the write). Touches only each field's own
/// bits; every other byte stays exactly as read.
pub fn apply_profile_settings(
    image: &mut [u8],
    schema_json: &str,
    settings_json: &str,
) -> Result<usize, String> {
    let options = schema_options(schema_json)?;
    let settings: Value = serde_json::from_str(settings_json)
        .map_err(|e| format!("invalid profile settings JSON: {e}"))?;
    let obj = settings
        .as_object()
        .ok_or("profile settings is not a JSON object")?;

    let mut applied = 0usize;
    for (key, value) in obj {
        let Some(loc) = tdh3_loc(key) else { continue };
        let ok = match loc {
            SLoc::Byte(n) => match resolve_index(key, value, &options) {
                Some(v) if SETTINGS_BASE + n < image.len() => {
                    image[SETTINGS_BASE + n] = v as u8;
                    true
                }
                _ => false,
            },
            SLoc::BrightnessByte(n) => match resolve_index(key, value, &options) {
                Some(v) if SETTINGS_BASE + n < image.len() => {
                    image[SETTINGS_BASE + n] = 4u8.saturating_sub((v as u8).min(4));
                    true
                }
                _ => false,
            },
            SLoc::Bits(n, lsb, w) => match resolve_index(key, value, &options) {
                Some(v) if SETTINGS_BASE + n < image.len() => {
                    set_bits(&mut image[SETTINGS_BASE + n], lsb, w, v as u8);
                    true
                }
                _ => false,
            },
            SLoc::Msg(i) => match value.as_str() {
                Some(s) if POWERON_MSG_BASE + i * MSG_LEN + MSG_LEN <= image.len() => {
                    encode_msg(image, i, s);
                    true
                }
                _ => false,
            },
        };
        if ok {
            applied += 1;
        }
    }
    Ok(applied)
}

#[derive(Serialize)]
pub struct Tdh3ProfileApplyResult {
    /// Number of profile fields actually written to the radio.
    pub applied: usize,
    /// Whether the post-write read-back matched what we intended to write.
    pub verified: bool,
    /// Set when verification could not run or found differences.
    pub verify_note: Option<String>,
    /// Absolute path of the pre-write backup `.img`.
    pub backup_path: String,
    /// The settings present on the radio after applying (read back) — lets the
    /// Radio Options form refresh to show what the profile set.
    pub settings: Tdh3Settings,
}

/// Apply a saved radio profile's non-channel settings to a connected TD-H3,
/// leaving channels untouched. Same safety model as the other writes: download +
/// back up the full image first, patch only the profile's settings bits, upload
/// the whole main range (channels and every unsupported setting preserved as
/// read), then read back and verify.
#[tauri::command]
pub async fn apply_tdh3_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    port: String,
    profile_id: i64,
) -> Result<Tdh3ProfileApplyResult, String> {
    // Pull the profile's model, schema, and saved settings up front (before the
    // blocking serial work, which can't hold the DB connection).
    let row: Option<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
        "SELECT rm.model, rm.non_channel_settings_schema, rp.non_channel_settings, rp.display_name \
         FROM radio_profiles rp JOIN radio_models rm ON rm.id = rp.radio_model_id WHERE rp.id = ?1",
    )
    .bind(profile_id)
    .fetch_optional(&state.pool)
    .await
    .estr()?;
    let (model, schema, settings, profile_name) = row.ok_or("radio profile not found")?;
    if model != "TD-H3" {
        return Err(format!(
            "This radio profile is for {model}, not the TD-H3."
        ));
    }
    let schema = schema.ok_or("the TD-H3 model has no settings schema")?;
    let settings = settings
        .ok_or("This profile has no saved settings to apply. Edit it under Radios first.")?;

    let backup_dir = app.path().app_data_dir().estr()?.join("radio-backups");
    std::fs::create_dir_all(&backup_dir).estr()?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let slug = slug_label(&profile_name);
    let backup_path = backup_dir.join(if slug.is_empty() {
        format!("tdh3-preprofile-{stamp}.img")
    } else {
        format!("tdh3-preprofile-{slug}-{stamp}.img")
    });

    tauri::async_runtime::spawn_blocking(move || {
        let mut p = open_port(&port)?;

        // 1. Download + back up the current radio contents.
        let ident = do_ident(&mut *p)?;
        let mut image = download(&mut *p, &ident)?;
        std::fs::write(&backup_path, &image)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        // 2. Patch the profile's settings bits into the downloaded image.
        let applied = apply_profile_settings(&mut image, &schema, &settings)?;

        // 3. Re-identify and upload the whole main range, then leave clone mode.
        std::thread::sleep(Duration::from_secs(1));
        reident(&mut *p)?;
        upload(&mut *p, &image)?;

        // 4. Read back and verify (non-fatal).
        let (verified, verify_note, result) = match verify_settings_after_write(&mut *p, &image) {
            Ok((ok, note, st)) => (ok, note, st),
            Err(e) => (
                false,
                Some(format!(
                    "Profile written, but read-back verification could not run ({e}). \
                     Power-cycle the radio and use Read to confirm."
                )),
                decode_settings(&image),
            ),
        };

        Ok(Tdh3ProfileApplyResult {
            applied,
            verified,
            verify_note,
            backup_path: backup_path.to_string_lossy().to_string(),
            settings: result,
        })
    })
    .await
    .estr()?
}

// ============================================================
// Phase C (cont.): import radio settings INTO a saved profile
// ============================================================
//
// The inverse of `apply_profile_settings`: read the radio's current settings and
// store them in a radio profile (update an existing one, merging so the deferred
// DTMF/key keys it already holds are kept, or create a new profile). The
// schema-keyed JSON shape matches what the profile's Settings tab edits.

/// Read the numeric byte index a non-`Msg` location holds.
fn read_index(image: &[u8], loc: &SLoc) -> i64 {
    match loc {
        SLoc::Byte(n) => *image.get(SETTINGS_BASE + n).unwrap_or(&0) as i64,
        SLoc::BrightnessByte(n) => {
            4i64 - (*image.get(SETTINGS_BASE + n).unwrap_or(&0)).min(4) as i64
        }
        SLoc::Bits(n, lsb, w) => bits(*image.get(SETTINGS_BASE + n).unwrap_or(&0), *lsb, *w) as i64,
        SLoc::Msg(_) => 0,
    }
}

/// Decode an image's settings into the schema-keyed JSON a profile stores
/// (booleans, select labels, integers, text) — the inverse of
/// [`apply_profile_settings`]. Only the keys we know how to locate are emitted.
pub fn decode_profile_settings(image: &[u8], schema_json: &str) -> Result<Value, String> {
    let schema: Value = serde_json::from_str(schema_json)
        .map_err(|e| format!("invalid settings schema JSON: {e}"))?;
    let arr = schema.as_array().ok_or("settings schema is not a JSON array")?;
    let mut map = serde_json::Map::new();
    for field in arr {
        let Some(key) = field.get("key").and_then(Value::as_str) else {
            continue;
        };
        let Some(loc) = tdh3_loc(key) else { continue };
        let ftype = field.get("type").and_then(Value::as_str).unwrap_or("text");
        let value = match loc {
            SLoc::Msg(i) => Value::String(decode_msg(image, i)),
            ref other => {
                let n = read_index(image, other);
                match ftype {
                    "boolean" => Value::Bool(n != 0),
                    "select" => {
                        let label = field
                            .get("options")
                            .and_then(Value::as_array)
                            .and_then(|opts| opts.get(n as usize))
                            .and_then(Value::as_str);
                        match label {
                            Some(s) => Value::String(s.to_string()),
                            None => continue, // out-of-range index; skip
                        }
                    }
                    _ => Value::Number(n.into()),
                }
            }
        };
        map.insert(key.to_string(), value);
    }
    Ok(Value::Object(map))
}

/// Save the (possibly edited) settings currently in the Radio Options form into a
/// radio profile — either updating an existing profile (merging over its stored
/// settings so keys we don't manage are preserved) or creating a new one. Pure
/// DB + CPU work; the radio was already read to populate the form.
#[tauri::command]
pub async fn save_tdh3_settings_to_profile(
    state: State<'_, AppState>,
    settings: Tdh3Settings,
    model_id: i64,
    profile_id: Option<i64>,
    new_name: Option<String>,
) -> Result<RadioProfile, String> {
    let schema: Option<String> =
        sqlx::query_scalar("SELECT non_channel_settings_schema FROM radio_models WHERE id = ?1")
            .bind(model_id)
            .fetch_optional(&state.pool)
            .await
            .estr()?
            .flatten();
    let schema = schema.ok_or("this radio model has no settings schema")?;

    // Render the form's settings to the schema-keyed JSON shape via a scratch
    // image (encode → decode), reusing the single source-of-truth bit map.
    let mut scratch = vec![0u8; MEMSIZE as usize + 0x20];
    encode_settings(&mut scratch, &settings);
    let decoded = decode_profile_settings(&scratch, &schema)?;
    let decoded_obj = decoded.as_object().cloned().unwrap_or_default();

    let id = if let Some(pid) = profile_id {
        // Merge the read keys over the profile's existing settings.
        let existing: Option<String> =
            sqlx::query_scalar("SELECT non_channel_settings FROM radio_profiles WHERE id = ?1")
                .bind(pid)
                .fetch_optional(&state.pool)
                .await
                .estr()?
                .flatten();
        let mut base = existing
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        for (k, v) in decoded_obj {
            base.insert(k, v);
        }
        let merged = serde_json::to_string(&Value::Object(base)).estr()?;
        sqlx::query(
            "UPDATE radio_profiles SET non_channel_settings = ?2, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        )
        .bind(pid)
        .bind(&merged)
        .execute(&state.pool)
        .await
        .estr()?;
        pid
    } else {
        let name = new_name
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or("Enter a name for the new profile.")?;
        let json = serde_json::to_string(&Value::Object(decoded_obj)).estr()?;
        sqlx::query(
            "INSERT INTO radio_profiles (display_name, radio_model_id, non_channel_settings, notes, updated_at) \
             VALUES (?1, ?2, ?3, NULL, CURRENT_TIMESTAMP)",
        )
        .bind(&name)
        .bind(model_id)
        .bind(&json)
        .execute(&state.pool)
        .await
        .estr()?
        .last_insert_rowid()
    };

    sqlx::query_as::<_, RadioProfile>(
        "SELECT id, display_name, radio_model_id, non_channel_settings, notes, created_at, updated_at FROM radio_profiles WHERE id = ?1",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .estr()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_channel() -> Channel {
        Channel {
            rx_freq: 462.5625,
            mode: Some("FM".into()),
            tone_mode: Some("off".into()),
            dcs_polarity: "NN".into(),
            cross_mode: String::new(),
            ..Default::default()
        }
    }

    #[test]
    fn freq_encode_round_trips_through_decode() {
        // Encode 462.5625 → lbcd → decode back via the read path's lbcd_to_u32.
        let b = freq_to_lbcd(462.5625);
        assert_eq!(lbcd_to_u32(&b) as f64 * 10.0 / 1_000_000.0, 462.5625);
    }

    #[test]
    fn bcd2_packs_little_endian_pairs() {
        assert_eq!(bcd2(885), [0x85, 0x08]); // CTCSS 88.5
        assert_eq!(bcd2(23), [0x23, 0x00]); // DTCS code 23
    }

    #[test]
    fn tone_encoding_matches_the_decoder() {
        // Tone (TX CTCSS only): rx off, tx 88.5 → decode summary "TX Tone 88.5".
        let mut c = base_channel();
        c.tone_mode = Some("Tone".into());
        c.ctcss_uplink = Some(88.5);
        let (rx, tx) = encode_tones(&c);
        let summary = decode_tone_summary(
            lbcd_to_u32(&rx),
            lbcd_to_u32(&tx),
        );
        assert_eq!(summary, "TX Tone 88.5");

        // DTCS normal both sides → byte[1] high bit 0x80, value 8023.
        let mut d = base_channel();
        d.tone_mode = Some("DTCS".into());
        d.dcs_code = Some("023".into());
        let (drx, dtx) = encode_tones(&d);
        assert_eq!(lbcd_to_u32(&dtx), 8023);
        assert_eq!(lbcd_to_u32(&drx), 8023);

        // DTCS reverse on the TX side → 12023.
        d.dcs_polarity = "RN".into();
        let (_r, t) = encode_tones(&d);
        assert_eq!(lbcd_to_u32(&t), 12023);

        // off → both fields 0xFFFF.
        let off = base_channel();
        assert_eq!(encode_tones(&off), ([0xFF, 0xFF], [0xFF, 0xFF]));
    }

    #[test]
    fn power_and_bandwidth_bits() {
        let mut c = base_channel();
        // FM + default power (unset → High, idx 2): byte14 = (2<<4)|(0<<3) = 0x20.
        assert_eq!(encode_channel(&c)[14], 0x20);
        // FM + explicit High (idx 2): same 0x20.
        c.power = Some("High".into());
        assert_eq!(encode_channel(&c)[14], 0x20);
        // FM + Med power (idx 1): byte14 = (1<<4)|(0<<3) = 0x10.
        c.power = Some("Med".into());
        assert_eq!(encode_channel(&c)[14], 0x10);
        // NFM + Low power (idx 0): byte14 = (0<<4)|(1<<3) = 0x08.
        c.mode = Some("NFM".into());
        c.power = Some("Low".into());
        assert_eq!(encode_channel(&c)[14], 0x08);
    }

    #[test]
    fn repeater_shift_comes_from_tx_freq_not_flags() {
        // A -0.6 MHz repeater channel. byte14 carries power/bandwidth only
        // (0x20 for High/FM) — the shift is carried ONLY by the TX-freq bytes,
        // exactly like the factory image (no flag bit encodes it).
        let mut c = base_channel();
        c.rx_freq = 146.82;
        c.tx_freq = Some(146.22);
        assert_eq!(encode_channel(&c)[14], 0x20);

        // The decoder derives the shift from the stored TX vs RX freqs.
        let slots = vec![SlotChannel { slot: 0, name: "RPT".into(), channel: c }];
        let mut image = vec![0xFFu8; 0x2008];
        patch_image(&mut image, &slots);
        assert_eq!(decode_channels(&image)[0].shift, "-0.600");
    }

    #[test]
    fn patch_writes_channel_1_and_clears_the_rest() {
        let mut image = vec![0x00u8; 0x2008];
        let slots = vec![SlotChannel {
            slot: 0, // → channel number 1
            name: "GMRS 01".into(),
            channel: {
                let mut c = base_channel();
                c.tx_freq = Some(462.5625);
                c
            },
        }];
        patch_image(&mut image, &slots);

        // Channel 1 lives at memory[1]; name at name index 0; used/scan bit 0 set.
        let decoded = decode_channels(&image);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].index, 1);
        assert_eq!(decoded[0].name, "GMRS 01");
        assert!((decoded[0].rx_mhz - 462.5625).abs() < 1e-6);
        assert_eq!(image[USEDFLAGS_BASE] & 0x01, 0x01);
        assert_eq!(image[SCANADD_BASE] & 0x01, 0x01);
        // memory[0] is unused and left untouched (patch writes channels 1..=199).
        assert_eq!(image[CHANNEL_BASE], 0x00);
        // Channel 2 (memory[2]) is cleared to 0xFF.
        assert_eq!(image[CHANNEL_BASE + 2 * ENTRY_LEN], 0xFF);
    }

    #[test]
    fn lbcd_decodes_little_endian_bcd() {
        // 146.520 MHz stored as freq/10 = 14652000 → lbcd bytes 00 20 65 14.
        assert_eq!(lbcd_to_u32(&[0x00, 0x20, 0x65, 0x14]), 14652000);
        // CTCSS 88.5 → 885.
        assert_eq!(lbcd_to_u32(&[0x85, 0x08]), 885);
        // Empty tone 0xFFFF → the 16665 sentinel.
        assert_eq!(lbcd_to_u32(&[0xFF, 0xFF]), 16665);
    }

    #[test]
    fn decode_tone_covers_all_modes() {
        assert_eq!(decode_tone_field(16665), "");
        assert_eq!(decode_tone_field(0), "");
        assert_eq!(decode_tone_field(885), "Tone 88.5");
        assert_eq!(decode_tone_field(8023), "DTCS 023 N");
        assert_eq!(decode_tone_field(12023), "DTCS 023 R");
    }

    #[test]
    fn tone_summary_combines_tx_and_rx() {
        assert_eq!(decode_tone_summary(16665, 16665), "—");
        // TX-only CTCSS (Tone mode): rx empty, tx set.
        assert_eq!(decode_tone_summary(16665, 885), "TX Tone 88.5");
        // Matched tone (TSQL): both equal → single label.
        assert_eq!(decode_tone_summary(885, 885), "Tone 88.5");
    }

    /// Build a minimal image with one programmed channel and assert the decoder
    /// reads it back faithfully (proves the buffer-offset map end-to-end).
    #[test]
    fn decodes_a_programmed_channel_from_an_image() {
        let mut image = vec![0xFFu8; 0x2008];
        // Channel 1 lives at memory[1] (memory[0] is unused) with its name at
        // name index 0 — the driver's one-slot offset.
        let off = CHANNEL_BASE + ENTRY_LEN;
        // rxfreq 146.520, txfreq 146.520 (simplex), lbcd freq/10.
        image[off..off + 4].copy_from_slice(&[0x00, 0x20, 0x65, 0x14]);
        image[off + 4..off + 8].copy_from_slice(&[0x00, 0x20, 0x65, 0x14]);
        // rxtone empty (0xFFFF), txtone 88.5 → Tone mode.
        image[off + 8..off + 10].copy_from_slice(&[0xFF, 0xFF]);
        image[off + 10..off + 12].copy_from_slice(&[0x85, 0x08]);
        // flags byte+14: lowpower=2 (High) at bits 5-4, wide=0 (FM).
        image[off + 14] = 0x20;
        // name "SIMPLEX " at name index 0 (channel 1's name).
        let name = b"SIMPLEX ";
        image[NAME_BASE..NAME_BASE + NAME_LEN].copy_from_slice(name);

        let decoded = decode_channels(&image);
        assert_eq!(decoded.len(), 1);
        let ch = &decoded[0];
        assert_eq!(ch.index, 1);
        assert_eq!(ch.name, "SIMPLEX");
        assert!((ch.rx_mhz - 146.520).abs() < 1e-6);
        assert_eq!(ch.tone, "TX Tone 88.5");
        assert_eq!(ch.power, "High");
        assert_eq!(ch.mode, "FM");
    }

    fn sample_settings() -> Tdh3Settings {
        Tdh3Settings {
            squelch: 3,
            brightness: 2, // index 2 → stored 4 - 2 = 2
            backlight: 1,
            vox_gain: 4,
            vox_delay: 1,
            tot: 5,
            scan_mode: 2,
            power_on_msg_mode: 1,
            power_save: 3,
            breath_led: 4,
            display_a: 1,
            display_b: 0,
            alarm_mode: 1,
            beep: true,
            voice_prompt: false,
            auto_lock: true,
            dual_watch: true,
            tx_220: true,
            tx_350: false,
            tx_500: true,
            am_band: true,
            power_on_msg1: "WW8L".into(),
            power_on_msg2: "TIM".into(),
            power_on_msg3: "73".into(),
        }
    }

    #[test]
    fn settings_round_trip_through_an_image() {
        // Start from a realistic image (0xFF fill) and confirm encode → decode
        // returns exactly what we put in for every field.
        let mut image = vec![0xFFu8; 0x2008];
        let st = sample_settings();
        encode_settings(&mut image, &st);
        assert_eq!(decode_settings(&image), st);
    }

    #[test]
    fn settings_encode_preserves_neighbouring_bits() {
        // The reserved/unknown bits packed alongside our fields must survive a
        // write untouched. Fill with 0xFF, encode all-zero-ish settings, and
        // check a byte we only partially own keeps its foreign bits.
        let mut image = vec![0xFFu8; 0x2008];
        let st = Tdh3Settings {
            scan_mode: 0,
            power_on_msg_mode: 0,
            beep: false,
            voice_prompt: false,
            auto_lock: false,
            dual_watch: false,
            display_b: 0,
            ..sample_settings()
        };
        encode_settings(&mut image, &st);
        // Byte 9 holds scanmode(7-6), keyautolock(4), btnvoice(2), voiceprompt(0)
        // plus unused16(5)/unused17(3)/unknown18(1) which started as 1s and must
        // remain set: 0b0010_1010 = 0x2A.
        assert_eq!(image[SETTINGS_BASE + 9], 0x2A);
    }

    #[test]
    fn brightness_inverts_between_stored_and_index() {
        let mut image = vec![0xFFu8; 0x2008];
        let mut st = sample_settings();
        st.brightness = 0; // brightest display → stored 4
        encode_settings(&mut image, &st);
        assert_eq!(image[SETTINGS_BASE + 5], 4);
        assert_eq!(decode_settings(&image).brightness, 0);
    }

    #[test]
    fn power_on_message_is_left_aligned_null_padded() {
        let mut image = vec![0xFFu8; 0x2008];
        let mut st = sample_settings();
        st.power_on_msg1 = "WW8L".into();
        encode_settings(&mut image, &st);
        assert_eq!(&image[POWERON_MSG_BASE..POWERON_MSG_BASE + 4], b"WW8L");
        assert_eq!(image[POWERON_MSG_BASE + 4], 0x00);
        assert_eq!(decode_settings(&image).power_on_msg1, "WW8L");
    }

    #[test]
    fn profile_settings_apply_by_schema_key() {
        // The TD-H3 schema keys (CHIRP names) with select labels + booleans +
        // text, applied to an image, must land at the right bits — and decode
        // back to the expected struct values (proves the schema→bits mapping).
        let schema = r#"[
            {"key":"squelch","type":"select","options":["0","1","2","3","4","5","6","7","8","9"]},
            {"key":"brightness","type":"select","options":["1","2","3","4","5"]},
            {"key":"ligcon","type":"select","options":["CONT","5s","10s","15s","30s"]},
            {"key":"voxgain","type":"select","options":["Off","1","2","3","4","5"]},
            {"key":"ponmsg","type":"select","options":["Off","Msg","Icon"]},
            {"key":"mdfa","type":"select","options":["Frequency","Name"]},
            {"key":"alarm","type":"select","options":["On site","Alarm"]},
            {"key":"btnvoice","type":"boolean"},
            {"key":"tx220","type":"boolean"},
            {"key":"poweron_msg.msg1","type":"text"},
            {"key":"ssidekey1","type":"select","options":["None","FM Radio"]}
        ]"#;
        let settings = r#"{
            "squelch":"7","brightness":"5","ligcon":"15s","voxgain":"3",
            "ponmsg":"Icon","mdfa":"Name","alarm":"Alarm","btnvoice":true,
            "tx220":true,"poweron_msg.msg1":"WW8L","ssidekey1":"FM Radio"
        }"#;
        let mut image = vec![0xFFu8; 0x2008];
        let applied = apply_profile_settings(&mut image, schema, settings).unwrap();
        // 10 known keys applied; the deferred key-assignment (ssidekey1) skipped.
        assert_eq!(applied, 10);

        let d = decode_settings(&image);
        assert_eq!(d.squelch, 7);
        assert_eq!(d.brightness, 4); // label "5" → index 4 (stored inverted as 0)
        assert_eq!(image[SETTINGS_BASE + 5], 0);
        assert_eq!(d.backlight, 3); // "15s" → index 3
        assert_eq!(d.vox_gain, 3);
        assert_eq!(d.power_on_msg_mode, 2); // "Icon"
        assert_eq!(d.display_a, 1); // "Name"
        assert_eq!(d.alarm_mode, 1); // "Alarm"
        assert!(d.beep);
        assert!(d.tx_220);
        assert_eq!(d.power_on_msg1, "WW8L");
    }

    #[test]
    fn decode_profile_settings_round_trips_apply() {
        // Apply a schema-keyed profile to an image, then decode it back out: the
        // emitted JSON must match the input for every supported key (the
        // radio→profile import is the faithful inverse of profile→radio apply).
        let schema = r#"[
            {"key":"squelch","type":"select","options":["0","1","2","3","4","5","6","7","8","9"]},
            {"key":"brightness","type":"select","options":["1","2","3","4","5"]},
            {"key":"ligcon","type":"select","options":["CONT","5s","10s","15s","30s"]},
            {"key":"ponmsg","type":"select","options":["Off","Msg","Icon"]},
            {"key":"mdfa","type":"select","options":["Frequency","Name"]},
            {"key":"btnvoice","type":"boolean"},
            {"key":"tx220","type":"boolean"},
            {"key":"poweron_msg.msg1","type":"text"},
            {"key":"ssidekey1","type":"select","options":["None","FM Radio"]}
        ]"#;
        let input = r#"{
            "squelch":"7","brightness":"5","ligcon":"15s","ponmsg":"Icon",
            "mdfa":"Name","btnvoice":true,"tx220":true,"poweron_msg.msg1":"WW8L"
        }"#;
        let mut image = vec![0xFFu8; 0x2008];
        apply_profile_settings(&mut image, schema, input).unwrap();

        let decoded = decode_profile_settings(&image, schema).unwrap();
        assert_eq!(decoded["squelch"], serde_json::json!("7"));
        assert_eq!(decoded["brightness"], serde_json::json!("5"));
        assert_eq!(decoded["ligcon"], serde_json::json!("15s"));
        assert_eq!(decoded["ponmsg"], serde_json::json!("Icon"));
        assert_eq!(decoded["mdfa"], serde_json::json!("Name"));
        assert_eq!(decoded["btnvoice"], serde_json::json!(true));
        assert_eq!(decoded["tx220"], serde_json::json!(true));
        assert_eq!(decoded["poweron_msg.msg1"], serde_json::json!("WW8L"));
        // The deferred key-assignment is not emitted (we don't locate it).
        assert!(decoded.get("ssidekey1").is_none());
    }

    #[test]
    fn profile_apply_skips_unknown_keys_and_preserves_neighbors() {
        // An unsupported key must be skipped, and applying one bitfield must not
        // disturb the reserved bits sharing its byte.
        let schema = r#"[{"key":"dbrx","type":"boolean"},{"key":"dtmfst","type":"boolean"}]"#;
        let mut image = vec![0xFFu8; 0x2008];
        let applied =
            apply_profile_settings(&mut image, schema, r#"{"dbrx":false,"dtmfst":true}"#).unwrap();
        assert_eq!(applied, 1); // only dbrx is mapped; dtmfst is deferred/skipped
        // byte 11: dbrx is bit 2 → cleared; the other bits stay 1 → 0xFB.
        assert_eq!(image[SETTINGS_BASE + 11], 0xFB);
    }

    #[test]
    fn set_bits_replaces_only_the_field() {
        let mut byte = 0xFFu8;
        set_bits(&mut byte, 6, 2, 0b01); // top two bits → 01
        assert_eq!(byte, 0b0111_1111);
        let mut byte = 0x00u8;
        set_bits(&mut byte, 2, 1, 1);
        assert_eq!(byte, 0b0000_0100);
    }
}
