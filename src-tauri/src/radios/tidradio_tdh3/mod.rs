//! TIDRADIO TD-H3 driver (`driver_key = "tidradio_tdh3"`).
//!
//! Speaks the TIDRADIO clone protocol — the same one CHIRP's
//! `chirp/drivers/tdh8.py` implements (shared TD-H8/H3 driver, H3 branch) — so
//! the app can read from and write to the radio without going through CHIRP.
//! The TD-H3's USB-C connector enumerates as a USB serial port. Moved here from
//! `commands/tdh3.rs` in Chunk 3.4 (mechanical extraction); the Tauri command
//! layer there still owns DB access, backup paths, and result DTOs.
//!
//! This module is the `ImageProgrammer` half (channels + full-image clone);
//! `settings.rs` is the settings half (`SettingsReader` + `SettingsWriter`).
//!
//! Protocol notes (ported verbatim from `tdh8.py`):
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

pub(crate) mod settings;

use std::time::Duration;

use serde::Serialize;
use serialport::{ClearBuffer, SerialPort};

use crate::commands::export;
use crate::commands::export::SlotChannel;
use crate::error::MapErrString;
use crate::models::{Channel, RadioModel};
use crate::radios::driver::{
    CodeplugProgramReport, DecodedChannelSample, ImageProgramRequest, ImageProgrammer, RadioDriver,
    RadioIdentity, SettingsReader, SettingsWriter,
};

const BAUD: u32 = 38400;
const TIMEOUT: Duration = Duration::from_secs(1);

/// TD-H3 identify magic (`tdh8.py` `TD_H3 = b'PVOJH\x5c\x14'`).
const MAGIC: &[u8] = &[0x50, 0x56, 0x4F, 0x4A, 0x48, 0x5C, 0x14];

/// `_memsize` for the H3: blocks are read 0x20 at a time from 0 up to this,
/// so the last block reads slightly past it to a 0x2000 radio span.
pub(crate) const MEMSIZE: u16 = 0x1FEF;
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
pub(crate) const MAX_CHANNEL: usize = 199;
/// Radio-address range written on upload (CHIRP `_ranges_main`); blocks step
/// 0x20, the last reading to a 0x2000 span.
const UPLOAD_END: u16 = 0x1FEF;

/// Minimum plausible image: the full ident-prefixed download (8-byte ident +
/// the 0x2000 radio span). A shorter buffer is not a TD-H3 image.
const MIN_IMAGE_LEN: usize = 0x2008;

/// Label map for the H3's 2-bit `lowpower` field. The H3 has THREE power levels
/// (CHIRP `tdh8.py` `_tx_power`): index 0 = Low (1W), 1 = Mid (4W), 2 = High
/// (8W). Uses the app's "Med" spelling so a decoded power round-trips.
const POWER_LEVELS: [&str; 3] = ["Low", "Med", "High"];

// ============================================================
// Driver registration (RadioDriver + ImageProgrammer + settings read/write)
// ============================================================

/// The TD-H3 driver unit type. All protocol state lives on the serial port; the
/// driver itself is stateless, so a single static instance serves the registry.
/// `SettingsReader`/`SettingsWriter` are implemented in `settings.rs`.
pub(crate) struct TidradioTdh3;

/// Registry entry (see `radios/registry.rs`). `identify`/`download_image` reach
/// it through the registry as of 3.6c and the settings read/write commands as
/// of 3.6d; the codeplug program command still calls the module functions below
/// directly until 3.6e.
pub(crate) static DRIVER: TidradioTdh3 = TidradioTdh3;

impl RadioDriver for TidradioTdh3 {
    fn key(&self) -> &'static str {
        "tidradio_tdh3"
    }

    fn display_name(&self) -> &'static str {
        "TIDRADIO TD-H3"
    }

    fn baud(&self) -> u32 {
        BAUD
    }

    fn identify(&self, port: &str) -> Result<RadioIdentity, String> {
        let mut p = open_port(port)?;
        let ident = do_ident(&mut *p)?;
        Ok(RadioIdentity {
            matched: ascii(&ident),
            ident_hex: hex(&ident),
            ident_ascii: Some(ascii(&ident)),
        })
    }

    fn as_image_programmer(&self) -> Option<&dyn ImageProgrammer> {
        Some(self)
    }

    fn as_settings_reader(&self) -> Option<&dyn SettingsReader> {
        Some(self)
    }

    fn as_settings_writer(&self) -> Option<&dyn SettingsWriter> {
        Some(self)
    }
}

impl ImageProgrammer for TidradioTdh3 {
    fn download_image(&self, port: &str) -> Result<(RadioIdentity, Vec<u8>), String> {
        let mut p = open_port(port)?;
        let ident = do_ident(&mut *p)?;
        let image = download(&mut *p, &ident)?;
        Ok((
            RadioIdentity {
                matched: ascii(&ident),
                ident_hex: hex(&ident),
                ident_ascii: Some(ascii(&ident)),
            },
            image,
        ))
    }

    fn decode_sample(&self, image: &[u8]) -> Vec<DecodedChannelSample> {
        decode_channels(image).into_iter().map(decoded_to_sample).collect()
    }

    fn upload_image(&self, port: &str, image: &[u8]) -> Result<(), String> {
        if image.len() < MIN_IMAGE_LEN {
            return Err(format!(
                "image is only {} bytes — not a TD-H3 image.",
                image.len()
            ));
        }
        let mut p = open_port(port)?;
        do_ident(&mut *p)?;
        upload(&mut *p, image)
    }

    fn build_image(
        &self,
        _model: &RadioModel,
        channels: &[SlotChannel],
        base: &[u8],
    ) -> Result<Vec<u8>, String> {
        if base.len() < MIN_IMAGE_LEN {
            return Err(format!(
                "base image is only {} bytes — not a TD-H3 image.",
                base.len()
            ));
        }
        if channels.len() > MAX_CHANNEL {
            return Err(format!(
                "{} channels exceed the TD-H3's {MAX_CHANNEL} memories.",
                channels.len()
            ));
        }
        let mut image = base.to_vec();
        patch_image(&mut image, channels);
        Ok(image)
    }

    /// Moved verbatim from `commands::tdh3::program_tdh3_codeplug` in Chunk
    /// 3.6e. Hardware-proven flow, unchanged: download + back up, patch
    /// channels/names and the used/scan bitmaps into THAT image, re-identify
    /// (the radio settles after a full read), upload the whole main range so
    /// every untouched byte — including all radio settings — goes back exactly
    /// as read, then verify.
    ///
    /// `req.settings` is ignored: the TD-H3 pushes profile settings through
    /// `SettingsWriter` as a separate, explicitly-acknowledged operation, and a
    /// channel program deliberately preserves the radio's own settings.
    fn program_codeplug(
        &self,
        port: &str,
        req: &ImageProgramRequest,
    ) -> Result<CodeplugProgramReport, String> {
        if req.channels.len() > MAX_CHANNEL {
            return Err(format!(
                "Codeplug has {} programmable channels, but the TD-H3 holds only {MAX_CHANNEL}.",
                req.channels.len()
            ));
        }
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let slug = slug_label(req.label);
        let backup_path = req.backup_dir.join(if slug.is_empty() {
            format!("tdh3-prewrite-{stamp}.img")
        } else {
            format!("tdh3-prewrite-{slug}-{stamp}.img")
        });

        let mut p = open_port(port)?;

        // 1. Download + back up the current radio contents.
        let ident = do_ident(&mut *p)?;
        let mut image = download(&mut *p, &ident)?;
        std::fs::write(&backup_path, &image)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        // 2. Patch channels/names + used/scan bitmaps into the downloaded image.
        let channels_written = req.channels.len();
        patch_image(&mut image, req.channels);

        // 3. Re-identify (the radio settles after a full read) and upload the
        //    whole main range, then exit programming mode.
        std::thread::sleep(Duration::from_secs(1));
        reident(&mut *p)?;
        upload(&mut *p, &image)?;

        // 4. Read back and verify (non-fatal — every block was ack'd).
        let (verified, note, channels) = match verify_after_write(&mut *p, &image) {
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

        Ok(CodeplugProgramReport {
            channels_written,
            slots_cleared: MAX_CHANNEL - channels_written,
            // A channel program leaves the radio's settings exactly as read.
            settings_written: None,
            verified: Some(verified),
            note,
            backup_path: backup_path.to_string_lossy().to_string(),
            channels: channels.into_iter().take(20).map(decoded_to_sample).collect(),
            zones_written: 0,
            zones_cleared: 0,
            scan_lists_written: 0,
            scan_lists_cleared: 0,
            contacts_written: 0,
            contacts_cleared: 0,
            expected_path: None,
            windows_written: Vec::new(),
            skipped: Vec::new(),
            warnings: Vec::new(),
        })
    }
}

/// Narrow the driver's own richer decode to the generic sanity-sample shape.
fn decoded_to_sample(c: Tdh3DecodedChannel) -> DecodedChannelSample {
    DecodedChannelSample {
        index: c.index,
        name: c.name,
        rx_mhz: c.rx_mhz,
        shift: Some(c.shift),
        tone: c.tone,
        power: c.power,
        mode: Some(c.mode),
    }
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
pub(crate) fn do_ident(p: &mut dyn SerialPort) -> Result<Vec<u8>, String> {
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
pub(crate) fn download(p: &mut dyn SerialPort, ident: &[u8]) -> Result<Vec<u8>, String> {
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

/// Decode programmed channels from an image for the sanity display. Empty slots
/// (rxfreq[0] == 0xFF, CHIRP's `get_raw()[0] == 0xff`) are skipped.
///
/// Channel numbers are 1..=199 (CHIRP `memory_bounds = (1, 199)`); `memory[0]`
/// is unused. The driver stores channel N at `memory[N]` but its NAME (and
/// scan/used flags) at `names[N-1]` — a deliberate one-slot offset, so the name
/// for slot `n` is read from name index `n - 1`.
pub(crate) fn decode_channels(image: &[u8]) -> Vec<Tdh3DecodedChannel> {
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

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Render printable bytes as ASCII, non-printables as '.'.
pub(crate) fn ascii(bytes: &[u8]) -> String {
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
// Backup filename helper (shared by the command layer)
// ============================================================

/// Slug a human label to a filesystem-safe, lowercase, dash-separated form
/// (empty if the label has no alphanumerics), capped to 40 chars.
pub(crate) fn slug_label(label: &str) -> String {
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

// ============================================================
// Write / upload
// ============================================================

/// Re-establish a clone session, retrying over a few seconds since the radio can
/// still be settling right after a full read.
pub(crate) fn reident(p: &mut dyn SerialPort) -> Result<Vec<u8>, String> {
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
pub(crate) fn verify_after_write(
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
pub(crate) fn patch_image(image: &mut [u8], slots: &[SlotChannel]) {
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
pub(crate) fn upload(p: &mut dyn SerialPort, image: &[u8]) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::driver::DriverCapabilities;

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
    fn capabilities_derive_image_and_settings_programmer() {
        let caps = DriverCapabilities::of(&DRIVER);
        assert!(caps.program_image);
        assert!(caps.read_settings);
        assert!(caps.write_settings);
        assert!(!caps.write_channels);
        assert!(!caps.write_callsign_db);
        assert!(!caps.export);
        assert!(!caps.diagnostics);
        assert_eq!(DRIVER.key(), "tidradio_tdh3");
        assert_eq!(DRIVER.baud(), 38400);
    }

    #[test]
    fn build_image_patches_base_without_hardware() {
        let base = vec![0xFFu8; MIN_IMAGE_LEN];
        let slots = vec![SlotChannel {
            slot: 0, // → channel number 1
            name: "SIMPLEX".to_string(),
            channel: {
                let mut c = base_channel();
                c.rx_freq = 146.520;
                c.tx_freq = Some(146.520);
                c
            },
        }];
        let model = RadioModel::default();
        let image = DRIVER.build_image(&model, &slots, &base).unwrap();
        let decoded = decode_channels(&image);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].name, "SIMPLEX");
        // A short base image is rejected, not sliced out of bounds.
        assert!(DRIVER.build_image(&model, &slots, &[0u8; 16]).is_err());
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

    #[test]
    fn slug_label_slugs_and_trims() {
        assert_eq!(slug_label("Weekend Trip!"), "weekend-trip");
        assert_eq!(slug_label("  TD-H3 / Home  "), "td-h3-home");
        assert_eq!(slug_label("!!!"), "");
    }
}
