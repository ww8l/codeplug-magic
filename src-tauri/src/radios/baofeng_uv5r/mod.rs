//! Baofeng UV-5R driver (`driver_key = "baofeng_uv5r"`).
//!
//! Speaks the Baofeng UV-5R clone protocol — the same one CHIRP's
//! `chirp/drivers/uv5r.py` implements — so the app can read from and write to
//! the radio without going through CHIRP. Moved here from
//! `commands/program.rs` in Chunk 3.3 (mechanical extraction); the Tauri
//! command layer there still owns DB access, backup paths, and result DTOs.
//!
//! Protocol notes (ported verbatim from `uv5r.py`):
//!   - serial: 9600 8N1, 1s timeout.
//!   - identify: send a 7-byte magic byte-by-byte (~10ms gaps); expect ack
//!     0x06; send 0x02; read 8 or 12 bytes terminated by 0xDD; send ack 0x06.
//!   - read block: send `S` + u16 addr (big-endian) + u8 size; reply is a 4-byte
//!     header `X` + addr + size, then `size` data bytes; then send ack 0x06.
//!   - the downloaded buffer begins with the 8-byte ident, so it is
//!     CHIRP-`.img`-compatible. That means radio address 0x0000 sits at BUFFER
//!     offset 0x0008: channels[128] live at buffer 0x0008 (16 bytes each) and
//!     names[128] at buffer 0x1008. `_memsize` (0x1808) counts the ident prefix.

pub(crate) mod settings;

use std::time::Duration;

use serde::Serialize;
use serialport::{ClearBuffer, SerialPort};

use crate::commands::export;
use crate::commands::export::SlotChannel;
use crate::error::MapErrString;
use crate::models::{Channel, RadioModel};
use crate::radios::driver::{
    CodeplugProgramReport, DecodedChannelSample, ImageProgramRequest, ImageProgrammer,
    RadioDriver, RadioIdentity,
};

const BAUD: u32 = 9600;
const TIMEOUT: Duration = Duration::from_secs(1);

/// UV-5R family identify magics (subset of `uv5r.py` `_idents`). Tried in order;
/// the original UV-5R and the "291"/UV-6-era firmwares cover the vast majority
/// of real radios. The label is reported back so the user can see which matched.
const IDENTS: &[(&str, &[u8])] = &[
    ("UV5R_ORIG", &[0x50, 0xBB, 0xFF, 0x01, 0x25, 0x98, 0x4D]),
    ("UV5R_291", &[0x50, 0xBB, 0xFF, 0x20, 0x12, 0x07, 0x25]),
    ("UV5R_UV6", &[0x50, 0xBB, 0xFF, 0x20, 0x12, 0x08, 0x23]),
];

// Buffer-offset memory map (image includes the 8-byte ident prefix).
const CHANNEL_BASE: usize = 0x0008;
const NAME_BASE: usize = 0x1008;
const ENTRY_LEN: usize = 16;
pub(crate) const CHANNEL_COUNT: usize = 128;
const NAME_LEN: usize = 7;

/// Minimum plausible image: ident prefix + main block + names (no aux).
pub(crate) const MIN_IMAGE_LEN: usize = 0x1808;

// Radio-address ranges we write on upload (buffer offset = addr + 8). We touch
// ONLY the channel array and the name array — never settings/aux — to keep the
// blast radius of a write as small as possible. Each spans 128 * 16 bytes.
pub(crate) const CHANNEL_ADDR: (u16, u16) = (0x0000, 0x0800);
pub(crate) const NAME_ADDR: (u16, u16) = (0x1000, 0x1800);

/// UV-5R DTCS/DCS code table = `sorted(chirp_common.DTCS_CODES + (645,))`. The
/// stored ul16 tone for a DCS channel is this index + 1 (normal polarity).
const UV5R_DTCS: [u16; 105] = [
    23, 25, 26, 31, 32, 36, 43, 47, 51, 53, 54, 65, 71, 72, 73, 74, 114, 115, 116, 122, 125, 131,
    132, 134, 143, 145, 152, 155, 156, 162, 165, 172, 174, 205, 212, 223, 225, 226, 243, 244, 245,
    246, 251, 252, 255, 261, 263, 265, 266, 271, 274, 306, 311, 315, 325, 331, 332, 343, 346, 351,
    356, 364, 365, 371, 411, 412, 413, 423, 431, 432, 445, 446, 452, 454, 455, 462, 464, 465, 466,
    503, 506, 516, 523, 526, 532, 546, 565, 606, 612, 624, 627, 631, 632, 645, 654, 662, 664, 703,
    712, 723, 731, 732, 734, 743, 754,
];

// ============================================================
// Driver registration (RadioDriver + ImageProgrammer)
// ============================================================

/// The UV-5R driver unit type. All protocol state lives on the serial port; the
/// driver itself is stateless, so a single static instance serves the registry.
pub(crate) struct BaofengUv5r;

/// Registry entry (see `radios/registry.rs`). `identify`/`download_image` reach
/// it through the registry as of 3.6c and `read_radio_settings` as of 3.6d; the
/// remaining program commands still call the module functions below directly
/// until 3.6e.
pub(crate) static DRIVER: BaofengUv5r = BaofengUv5r;

impl RadioDriver for BaofengUv5r {
    fn key(&self) -> &'static str {
        "baofeng_uv5r"
    }

    fn display_name(&self) -> &'static str {
        "Baofeng UV-5R"
    }

    fn baud(&self) -> u32 {
        BAUD
    }

    fn identify(&self, port: &str) -> Result<RadioIdentity, String> {
        let mut p = open_port(port)?;
        let (matched, ident) = ident_radio(&mut *p)?;
        Ok(RadioIdentity {
            matched,
            ident_hex: hex(&ident),
            // The UV-5R ident is a binary magic, not a printable model token.
            ident_ascii: None,
        })
    }

    fn as_image_programmer(&self) -> Option<&dyn ImageProgrammer> {
        Some(self)
    }

    /// Read only: the UV-5R has no standalone settings-write path — its profile
    /// settings are encoded into the image `program_codeplug` uploads, never
    /// pushed on their own. Hence [`SettingsReader`] without [`SettingsWriter`].
    fn as_settings_reader(&self) -> Option<&dyn crate::radios::driver::SettingsReader> {
        Some(self)
    }
}

impl ImageProgrammer for BaofengUv5r {
    fn download_image(&self, port: &str) -> Result<(RadioIdentity, Vec<u8>), String> {
        let mut p = open_port(port)?;
        let (matched, ident) = ident_radio(&mut *p)?;
        let image = download(&mut *p, &ident)?;
        Ok((
            RadioIdentity {
                matched,
                ident_hex: hex(&ident),
                ident_ascii: None,
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
                "image is only {} bytes — not a UV-5R image.",
                image.len()
            ));
        }
        let mut p = open_port(port)?;
        let (magic, _ident) = ident_radio(&mut *p)?;
        if !magic.starts_with("UV5R") {
            return Err("the connected radio did not identify as a UV-5R".into());
        }
        upload_full_image(&mut *p, image)
    }

    fn build_image(
        &self,
        _model: &RadioModel,
        channels: &[SlotChannel],
        base: &[u8],
    ) -> Result<Vec<u8>, String> {
        if base.len() < MIN_IMAGE_LEN {
            return Err(format!(
                "base image is only {} bytes — not a UV-5R image.",
                base.len()
            ));
        }
        if channels.len() > CHANNEL_COUNT {
            return Err(format!(
                "{} channels exceed the UV-5R's {CHANNEL_COUNT} memories.",
                channels.len()
            ));
        }
        let mut image = base.to_vec();
        patch_image(&mut image, channels);
        Ok(image)
    }

    /// Moved verbatim from `commands::program::program_codeplug` in Chunk 3.6e.
    /// Hardware-proven flow, unchanged: download + back up the current image,
    /// patch channels/names (and the profile's settings, if any) into THAT
    /// image so every untouched byte goes back exactly as read, upload, verify.
    ///
    /// Which ranges get uploaded depends on whether settings are written —
    /// CHIRP's full main + aux ranges when they are, channels + names alone when
    /// they are not. Keeping the blast radius that small is deliberate.
    fn program_codeplug(
        &self,
        port: &str,
        req: &ImageProgramRequest,
    ) -> Result<CodeplugProgramReport, String> {
        if req.channels.len() > CHANNEL_COUNT {
            return Err(format!(
                "Codeplug has {} programmable channels, but the UV-5R holds only {CHANNEL_COUNT}.",
                req.channels.len()
            ));
        }
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let backup_path = req
            .backup_dir
            .join(backup_filename("prewrite", req.label, &stamp));

        let mut p = open_port(port)?;

        // 1. Download + back up the current radio contents.
        let (magic, ident) = ident_radio(&mut *p)?;
        if !magic.starts_with("UV5R") {
            return Err("the connected radio did not identify as a UV-5R".into());
        }
        let mut image = download(&mut *p, &ident)?;
        std::fs::write(&backup_path, &image)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        // 2. Patch the channel + name regions in the downloaded image.
        let channels_written = req.channels.len();
        patch_image(&mut image, req.channels);

        // 2b. Patch the radio profile's non-channel settings into the image, if
        //     the profile carries them. This makes the profile authoritative:
        //     every editable setting is written from the profile.
        let settings_written = match req.settings {
            Some((settings, schema)) => {
                let settings_json = serde_json::to_string(settings).estr()?;
                Some(settings::apply_profile_settings(
                    &mut image,
                    schema,
                    &settings_json,
                )?)
            }
            None => None,
        };

        // 3. Upload the patched regions (re-identify to start a clone session).
        //    The radio may take a moment to be ready to talk again after the
        //    full read, so settle first, then retry the identify.
        std::thread::sleep(Duration::from_secs(1));
        reident_with_retry(&mut *p)?;
        if settings_written.is_some() {
            for &(start, end) in settings::SETTINGS_MAIN_RANGES {
                write_region(&mut *p, &image, start, end)?;
            }
            for &(start, end) in settings::SETTINGS_AUX_RANGES {
                write_aux_region(&mut *p, &image, start, end)?;
            }
        } else {
            write_region(&mut *p, &image, CHANNEL_ADDR.0, CHANNEL_ADDR.1)?;
            write_region(&mut *p, &image, NAME_ADDR.0, NAME_ADDR.1)?;
        }

        // 4. Read back and verify (non-fatal: a write that ack'd every block
        //    succeeded; verification is a best-effort confirmation).
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
            slots_cleared: CHANNEL_COUNT - channels_written,
            settings_written,
            verified: Some(verified),
            note,
            backup_path: backup_path.to_string_lossy().to_string(),
            channels: channels.into_iter().map(decoded_to_sample).collect(),
            // The UV-5R programs channels + settings only, and rewrites whole
            // ranges rather than addressed windows.
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
fn decoded_to_sample(c: DecodedChannel) -> DecodedChannelSample {
    DecodedChannelSample {
        index: c.index,
        name: c.name,
        rx_mhz: c.rx_mhz,
        // The UV-5R decoder reports neither shift nor bandwidth.
        shift: None,
        tone: c.tone,
        power: c.power,
        mode: None,
    }
}

// ============================================================
// Serial protocol (ported from CHIRP uv5r.py)
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

/// Read exactly `n` bytes (looping across timeouts up to a bounded number of
/// empty reads), erroring if the radio goes quiet mid-block.
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

/// Handshake with one magic. Returns the 8-byte ident on success.
fn do_ident(p: &mut dyn SerialPort, magic: &[u8], secondack: bool) -> Result<Vec<u8>, String> {
    let _ = p.clear(ClearBuffer::All);
    for b in magic {
        p.write_all(&[*b]).estr()?;
        std::thread::sleep(Duration::from_millis(10));
    }
    if read_byte(p)? != Some(0x06) {
        return Err("no response to identify (radio off, wrong cable, or not a UV-5R?)".into());
    }
    p.write_all(&[0x02]).estr()?;

    let mut response = Vec::new();
    for _ in 0..12 {
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
    if response.len() != 8 && response.len() != 12 {
        return Err(format!(
            "unexpected ident ({} bytes): {}",
            response.len(),
            hex(&response)
        ));
    }
    // 12-byte responses are compressed to the canonical 8-byte ident.
    let ident = if response.len() == 12 {
        let mut v = vec![response[0], response[3], response[5]];
        v.extend_from_slice(&response[7..]);
        v
    } else {
        response
    };

    if secondack {
        p.write_all(&[0x06]).estr()?;
        if read_byte(p)? != Some(0x06) {
            return Err("radio refused clone (no second ack)".into());
        }
    }
    Ok(ident)
}

/// Try each known magic until one identifies the radio.
pub(crate) fn ident_radio(p: &mut dyn SerialPort) -> Result<(String, Vec<u8>), String> {
    let mut last = String::from("no contact with radio");
    for (name, magic) in IDENTS {
        match do_ident(p, magic, true) {
            Ok(ident) => return Ok((name.to_string(), ident)),
            Err(e) => {
                last = e;
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
    Err(format!("could not identify radio: {last}"))
}

/// Re-establish a clone session after a step that may leave the radio briefly
/// busy or rebooting. Unlike CHIRP — where download and upload are separate
/// clicks with seconds of human delay between them — we chain read→write→verify
/// automatically, so the radio can still be settling when we try to talk again.
/// Retry the identify sequence over a window of several seconds instead of
/// giving up after a single pass (which was the cause of the spurious
/// "no response to identify" right after a successful read).
pub(crate) fn reident_with_retry(p: &mut dyn SerialPort) -> Result<(String, Vec<u8>), String> {
    const ATTEMPTS: usize = 5;
    let mut last = String::from("no contact with radio");
    for attempt in 0..ATTEMPTS {
        let _ = p.clear(ClearBuffer::All);
        match ident_radio(p) {
            Ok(found) => return Ok(found),
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
         If the radio rebooted out of clone mode, put it back in clone mode and retry."
    ))
}

/// Read one memory block. `first_command` skips the leading-ack read used for
/// the very first block after identify.
fn read_block(
    p: &mut dyn SerialPort,
    start: u16,
    size: u8,
    first_command: bool,
) -> Result<Vec<u8>, String> {
    let msg = [b'S', (start >> 8) as u8, (start & 0xFF) as u8, size];
    p.write_all(&msg).estr()?;

    if !first_command && read_byte(p)? != Some(0x06) {
        return Err(format!("radio refused block 0x{start:04x} (no ack)"));
    }

    let header = read_exact(p, 4)?;
    let cmd = header[0];
    let addr = u16::from_be_bytes([header[1], header[2]]);
    let length = header[3];
    if cmd != b'X' || addr != start || length != size {
        return Err(format!(
            "bad header for block 0x{start:04x}: cmd={cmd:#04x} addr={addr:#06x} len={length:#04x}"
        ));
    }

    let chunk = read_exact(p, size as usize)?;
    p.write_all(&[0x06]).estr()?;
    std::thread::sleep(Duration::from_millis(50));
    Ok(chunk)
}

/// Faithful port of `_do_download` for an aux-block radio (the UV-5R). Returns
/// the full image buffer (ident prefix + main 0x0000..0x1800 + aux block).
pub(crate) fn download(p: &mut dyn SerialPort, ident: &[u8]) -> Result<Vec<u8>, String> {
    let mut data = ident.to_vec();

    // Firmware-version probe (not appended): first read uses first_command=true.
    // block2[15] == 0xFF flags the known "dropped byte" issue, which changes how
    // the tail of the aux block must be read.
    let _block0 = read_block(p, 0x1E80, 0x40, true)?;
    let _block1 = read_block(p, 0x1EC0, 0x40, false)?;
    let block2 = read_block(p, 0x1FC0, 0x40, false)?;
    let dropped = block2.get(15) == Some(&0xFF);

    // Main block.
    let mut addr = 0x0000u16;
    while addr < 0x1800 {
        data.extend(read_block(p, addr, 0x40, false)?);
        addr += 0x40;
    }

    // Aux block.
    if dropped {
        let mut a = 0x1EC0u16;
        while a < 0x1FC0 {
            data.extend(read_block(p, a, 0x40, false)?);
            a += 0x40;
        }
        let mut a = 0x1FC0u16;
        while a < 0x2000 {
            data.extend(read_block(p, a, 0x10, false)?);
            a += 0x10;
        }
    } else {
        let mut a = 0x1EC0u16;
        while a < 0x2000 {
            data.extend(read_block(p, a, 0x40, false)?);
            a += 0x40;
        }
    }

    Ok(data)
}

// ============================================================
// Decoding (read-only sanity sample)
// ============================================================

/// One decoded channel row, shown to the user as a sanity sample after a
/// download or program pass.
#[derive(Serialize, PartialEq)]
pub struct DecodedChannel {
    pub index: usize,
    pub name: String,
    pub rx_mhz: f64,
    /// Human-readable tone summary decoded from the channel bytes, e.g.
    /// "T 88.5", "TSQL 100.0", "DTCS 023 N", "Cross Tone->DTCS", or "—".
    pub tone: String,
    /// "High" or "Low" (the radio's stored TX power level).
    pub power: String,
}

/// Decode programmed channels from an image for a sanity display. Empty slots
/// (rxfreq all 0xFF) are skipped.
pub(crate) fn decode_channels(image: &[u8]) -> Vec<DecodedChannel> {
    let mut out = Vec::new();
    for i in 0..CHANNEL_COUNT {
        let off = CHANNEL_BASE + i * ENTRY_LEN;
        if off + 4 > image.len() {
            break;
        }
        let rx = &image[off..off + 4];
        if rx.iter().all(|&b| b == 0xFF) {
            continue; // unprogrammed slot
        }
        let rx_mhz = lbcd_to_u32(rx) as f64 * 10.0 / 1_000_000.0;
        let name = decode_name(image, i);
        let rxtone = u16::from_le_bytes([image[off + 8], image[off + 9]]);
        let txtone = u16::from_le_bytes([image[off + 10], image[off + 11]]);
        let tone = decode_tone_summary(rxtone, txtone);
        let power = if image[off + 14] & 0x03 == 0 { "High" } else { "Low" };
        out.push(DecodedChannel {
            index: i,
            name,
            rx_mhz,
            tone,
            power: power.to_string(),
        });
    }
    out
}

fn decode_name(image: &[u8], i: usize) -> String {
    let off = NAME_BASE + i * ENTRY_LEN;
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

/// Little-endian BCD (CHIRP `lbcd`): byte[0] is the least-significant digit
/// pair; within a byte the high nibble is tens, low nibble is ones.
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

// ============================================================
// Write / upload
// ============================================================

/// Build a backup `.img` filename like `uv5r-prewrite-weekend-trip-20260619-101530.img`.
/// `label` is a human name (codeplug or profile) slugged to a filesystem-safe,
/// lowercase, dash-separated form; an empty/symbol-only label is omitted so the
/// name falls back to `uv5r-{kind}-{stamp}.img`.
pub(crate) fn backup_filename(kind: &str, label: &str, stamp: &impl std::fmt::Display) -> String {
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
    let slug: String = slug.trim_end_matches('-').chars().take(40).collect();
    if slug.is_empty() {
        format!("uv5r-{kind}-{stamp}.img")
    } else {
        format!("uv5r-{kind}-{slug}-{stamp}.img")
    }
}

/// Re-read the radio and compare the channel/name regions (by decoded content,
/// which is robust to the radio massaging display-icon bits on save).
pub(crate) fn verify_after_write(
    p: &mut dyn SerialPort,
    intended: &[u8],
) -> Result<(bool, Option<String>, Vec<DecodedChannel>), String> {
    std::thread::sleep(Duration::from_secs(1));
    let (_m, ident) = reident_with_retry(p)?;
    let readback = download(p, &ident)?;

    let expected = decode_channels(intended);
    let actual = decode_channels(&readback);
    if expected == actual {
        Ok((true, None, actual))
    } else {
        let note = format!(
            "Read-back shows {} channels; {} were intended. The backup is saved if you need to revert.",
            actual.len(),
            expected.len()
        );
        Ok((false, Some(note), actual))
    }
}

/// Overwrite the channel + name regions of `image` so each included channel
/// lands in its preserved slot and every other slot (codeplug gaps from excluded
/// channels, plus trailing memories) is cleared (0xFF = empty). The result makes
/// the radio match the codeplug exactly while keeping channel positions stable.
pub(crate) fn patch_image(image: &mut [u8], slots: &[SlotChannel]) {
    // Index the included channels by their preserved slot.
    let mut assigned: Vec<Option<&SlotChannel>> = vec![None; CHANNEL_COUNT];
    for s in slots {
        if s.slot < CHANNEL_COUNT {
            assigned[s.slot] = Some(s);
        }
    }

    for i in 0..CHANNEL_COUNT {
        let co = CHANNEL_BASE + i * ENTRY_LEN;
        let no = NAME_BASE + i * ENTRY_LEN;
        if let Some(s) = assigned[i] {
            image[co..co + ENTRY_LEN].copy_from_slice(&encode_channel(&s.channel));
            // Names: write the 7 char cells; leave the trailing 9 bytes as read
            // (matches CHIRP, which only touches name[0..7]).
            image[no..no + NAME_LEN].copy_from_slice(&name_bytes(&s.name));
        } else {
            image[co..co + ENTRY_LEN].fill(0xFF);
            image[no..no + ENTRY_LEN].fill(0xFF);
        }
    }
}

/// Encode one 16-byte UV-5R channel block. Starts from a zeroed block (as CHIRP
/// does) then sets the defined fields.
fn encode_channel(c: &Channel) -> [u8; ENTRY_LEN] {
    let mut m = [0u8; ENTRY_LEN];

    m[0..4].copy_from_slice(&freq_to_lbcd(c.rx_freq));
    m[4..8].copy_from_slice(&freq_to_lbcd(export::tx_frequency(c)));

    let (rxtone, txtone) = encode_tones(c);
    m[8..10].copy_from_slice(&rxtone.to_le_bytes());
    m[10..12].copy_from_slice(&txtone.to_le_bytes());

    // byte 12 (unused1:3, isuhf:1, scode:4) and byte 13 stay 0, as CHIRP leaves
    // them for a freshly written channel.

    // byte 14: low 2 bits = lowpower (0 = High/4W, 1 = Low/1W).
    m[14] = encode_power(c);

    // byte 15: bit6 = wide (FM=1, NFM=0), bit2 = scan (1 = include in scan).
    let wide: u8 = if c.mode.as_deref().map(|s| s.eq_ignore_ascii_case("NFM")) == Some(true) {
        0
    } else {
        1
    };
    m[15] = (wide << 6) | (1 << 2);

    m
}

/// 7-byte name cell: ASCII (uppercased), unused cells padded with 0xFF.
fn name_bytes(name: &str) -> [u8; NAME_LEN] {
    let mut out = [0xFFu8; NAME_LEN];
    for (i, ch) in name.to_uppercase().chars().take(NAME_LEN).enumerate() {
        out[i] = if ch.is_ascii() { ch as u8 } else { 0xFF };
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

/// byte 14 lowpower:2 — 0 = High (max), 1 = Low. The master DB power level is
/// radio-agnostic; for the (two-level) UV-5R only "Low" drops power. "Med" (4 W on
/// tri-power radios) equals the UV-5R's High (4 W), so it maps to High here.
/// Tri-power UV-5R variants (8 W high) aren't handled yet.
fn encode_power(c: &Channel) -> u8 {
    if c.power.as_deref().map(|s| s.eq_ignore_ascii_case("Low")) == Some(true) {
        1
    } else {
        0
    }
}

/// Encode the CTCSS uplink/downlink (TX/RX) tone as the radio's freq*10 value.
fn ctcss_val(t: Option<f64>) -> u16 {
    (t.unwrap_or(0.0) * 10.0).round() as u16
}

/// Encode a DTCS code string ("023") as the radio's 1-based UV5R_DTCS index
/// (0 if unknown/empty). Reverse polarity adds 0x69 (handled by the caller).
fn dtcs_val(code: &Option<String>) -> u16 {
    code.as_deref()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .and_then(|n| UV5R_DTCS.iter().position(|&c| c == n))
        .map(|i| i as u16 + 1)
        .unwrap_or(0)
}

/// Split a cross-mode string ("DTCS->Tone") into (tx_mode, rx_mode). Empty sides
/// ("->Tone", "DTCS->") are allowed and decode to "".
fn parse_cross(cross_mode: &str) -> (&str, &str) {
    cross_mode.split_once("->").unwrap_or(("", ""))
}

/// (rxtone, txtone) ul16 pair, using the UV-5R tone encoding (ported from CHIRP
/// `uv5r.py`). tone_mode follows CHIRP's universal scheme:
///   off  -> both 0
///   Tone -> TX CTCSS (uplink) only, RX open
///   TSQL -> TX = RX CTCSS (downlink/ctone)
///   DTCS -> TX = RX DTCS (dcs_code), per-side polarity from dcs_polarity
///   Cross-> per-side: each side is Tone (uplink/downlink CTCSS), DTCS
///           (dcs_code TX / dcs_rx_code RX), or empty (open)
/// CTCSS stores freq*10 (>= 0x258); DTCS stores the 1-based index, +0x69 for
/// reverse polarity. Polarity[0] = TX, polarity[1] = RX.
fn encode_tones(c: &Channel) -> (u16, u16) {
    let pol = c.dcs_polarity.as_bytes();
    let tx_rev = pol.first() == Some(&b'R');
    let rx_rev = pol.get(1) == Some(&b'R');
    let rev = |v: u16, reverse: bool| if v != 0 && reverse { v + 0x69 } else { v };

    let mode = c.tone_mode.as_deref().unwrap_or("off");
    if mode.eq_ignore_ascii_case("Tone") {
        (0, ctcss_val(c.ctcss_uplink))
    } else if mode.eq_ignore_ascii_case("TSQL") {
        let t = ctcss_val(c.ctcss_downlink);
        (t, t)
    } else if mode.eq_ignore_ascii_case("DTCS") {
        let v = dtcs_val(&c.dcs_code);
        (rev(v, rx_rev), rev(v, tx_rev))
    } else if mode.eq_ignore_ascii_case("Cross") {
        let (txmode, rxmode) = parse_cross(&c.cross_mode);
        let tx = if txmode.eq_ignore_ascii_case("Tone") {
            ctcss_val(c.ctcss_uplink)
        } else if txmode.eq_ignore_ascii_case("DTCS") {
            rev(dtcs_val(&c.dcs_code), tx_rev)
        } else {
            0
        };
        let rx = if rxmode.eq_ignore_ascii_case("Tone") {
            ctcss_val(c.ctcss_downlink)
        } else if rxmode.eq_ignore_ascii_case("DTCS") {
            rev(dtcs_val(&c.dcs_rx_code), rx_rev)
        } else {
            0
        };
        (rx, tx)
    } else {
        (0, 0)
    }
}

/// Decode a single stored tone field (rxtone or txtone) into a human label.
/// 0/0xFFFF = open; >= 0x258 = CTCSS freq*10; otherwise a 1-based DTCS index
/// (1..=105 normal, 0x6A.. reverse, per CHIRP `uv5r.py`).
fn decode_tone_field(v: u16) -> String {
    if v == 0 || v == 0xFFFF {
        String::new()
    } else if v >= 0x258 {
        format!("{:.1}", v as f64 / 10.0)
    } else if v <= 105 {
        format!("D{:03} N", UV5R_DTCS[(v - 1) as usize])
    } else {
        let idx = (v.saturating_sub(0x6A)) as usize;
        let code = UV5R_DTCS.get(idx).copied().unwrap_or(0);
        format!("D{code:03} R")
    }
}

/// Build the read-back tone summary shown in the program-result table.
fn decode_tone_summary(rxtone: u16, txtone: u16) -> String {
    let tx = decode_tone_field(txtone);
    let rx = decode_tone_field(rxtone);
    match (tx.is_empty(), rx.is_empty()) {
        (true, true) => "—".to_string(),
        // TX CTCSS only -> Tone
        (false, true) if !tx.starts_with('D') => format!("T {tx}"),
        // both equal CTCSS -> TSQL
        (false, false) if tx == rx && !tx.starts_with('D') => format!("TSQL {tx}"),
        // both DTCS, same -> DTCS
        (false, false) if tx == rx && tx.starts_with('D') => format!("DTCS {}", &tx[1..]),
        // anything mixed -> show both sides
        _ => {
            let l = if tx.is_empty() { "off".into() } else { tx };
            let r = if rx.is_empty() { "off".into() } else { rx };
            format!("Cross {l}->{r}")
        }
    }
}

/// Upload one main-block radio-address range [start, end) in 0x10-byte blocks.
pub(crate) fn write_region(
    p: &mut dyn SerialPort,
    image: &[u8],
    start: u16,
    end: u16,
) -> Result<(), String> {
    let mut addr = start;
    while addr < end {
        let off = addr as usize + 8; // buffer offset = radio address + ident prefix
        send_block(p, addr, &image[off..off + 0x10])?;
        addr += 0x10;
    }
    Ok(())
}

/// Upload one aux-block radio-address range [start, end) in 0x10-byte blocks.
/// The aux block was read starting at radio 0x1EC0 and stored at image 0x1808,
/// so the image offset is `0x1808 + (addr - 0x1EC0)` rather than `addr + 8`.
pub(crate) fn write_aux_region(
    p: &mut dyn SerialPort,
    image: &[u8],
    start: u16,
    end: u16,
) -> Result<(), String> {
    let mut addr = start;
    while addr < end {
        let off = settings::AUX_IMAGE_BASE + (addr - settings::AUX_RADIO_BASE) as usize;
        if off + 0x10 > image.len() {
            return Err(format!(
                "aux block 0x{addr:04x} is past the downloaded image (no aux data read)"
            ));
        }
        send_block(p, addr, &image[off..off + 0x10])?;
        addr += 0x10;
    }
    Ok(())
}

/// Write a single block: `X` + addr(BE u16) + len + data, then expect ack 0x06.
fn send_block(p: &mut dyn SerialPort, addr: u16, data: &[u8]) -> Result<(), String> {
    let mut msg = vec![b'X', (addr >> 8) as u8, (addr & 0xFF) as u8, data.len() as u8];
    msg.extend_from_slice(data);
    p.write_all(&msg).estr()?;
    std::thread::sleep(Duration::from_millis(50));
    if read_byte(p)? != Some(0x06) {
        return Err(format!("radio refused to accept block 0x{addr:04x}"));
    }
    Ok(())
}

/// Write a full image to an already-identified radio: CHIRP's main ranges
/// (channels, names, DTMF, settings) plus the aux ranges when the image carries
/// the aux block. This is the restore path — everything we ever touch is
/// written straight from the image.
pub(crate) fn upload_full_image(p: &mut dyn SerialPort, image: &[u8]) -> Result<(), String> {
    for &(start, end) in settings::SETTINGS_MAIN_RANGES {
        write_region(p, image, start, end)?;
    }
    // Aux ranges only if the image actually contains the aux block.
    let aux_end = settings::AUX_IMAGE_BASE + (0x1FD0 - settings::AUX_RADIO_BASE as usize);
    if image.len() >= aux_end {
        for &(start, end) in settings::SETTINGS_AUX_RANGES {
            write_aux_region(p, image, start, end)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::driver::DriverCapabilities;

    #[test]
    fn capabilities_derive_image_programmer_and_settings_read() {
        let caps = DriverCapabilities::of(&DRIVER);
        assert!(caps.program_image);
        assert!(caps.read_settings);
        // Read-only settings: the UV-5R's profile settings go out inside the
        // image `program_codeplug` uploads, never as a standalone push. This is
        // what the `SettingsReader`/`SettingsWriter` split (3.6d) exists to say.
        assert!(!caps.write_settings);
        assert!(!caps.write_channels);
        assert!(!caps.write_callsign_db);
        assert!(!caps.export);
        assert!(!caps.diagnostics);
        assert_eq!(DRIVER.key(), "baofeng_uv5r");
        assert_eq!(DRIVER.baud(), 9600);
    }

    #[test]
    fn build_image_patches_base_without_hardware() {
        let base = vec![0x00u8; MIN_IMAGE_LEN];
        let slots = vec![SlotChannel {
            slot: 0,
            name: "SIMPLEX".to_string(),
            channel: Channel {
                rx_freq: 146.520,
                mode: Some("FM".into()),
                ..Default::default()
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
    fn lbcd_decodes_known_frequency() {
        // 146.520 MHz -> freq/10 = 14_652_000 -> lbcd little-endian pairs.
        let bytes = [0x00u8, 0x20, 0x65, 0x14];
        assert_eq!(lbcd_to_u32(&bytes), 14_652_000);
        let mhz = lbcd_to_u32(&bytes) as f64 * 10.0 / 1_000_000.0;
        assert!((mhz - 146.520).abs() < 1e-6);
    }

    #[test]
    fn backup_filename_slugs_label() {
        assert_eq!(
            backup_filename("prewrite", "Weekend Trip!", &"20260619-101530"),
            "uv5r-prewrite-weekend-trip-20260619-101530.img"
        );
        // Collapses runs of symbols/spaces and trims trailing dashes.
        assert_eq!(
            backup_filename("settings", "  UV-5R / Home  ", &"S"),
            "uv5r-settings-uv-5r-home-S.img"
        );
        // Symbol-only label falls back to the unlabeled form.
        assert_eq!(
            backup_filename("prewrite", "!!!", &"S"),
            "uv5r-prewrite-S.img"
        );
    }

    #[test]
    fn empty_slots_are_skipped() {
        // A blank radio reads back as all-0xFF; every channel slot is empty.
        let img = vec![0xFFu8; 0x1808];
        assert!(decode_channels(&img).is_empty());
    }

    #[test]
    fn decodes_one_programmed_channel_with_name() {
        let mut img = vec![0xFFu8; 0x1808];
        // Channel 0 rxfreq = 146.520.
        img[CHANNEL_BASE..CHANNEL_BASE + 4].copy_from_slice(&[0x00, 0x20, 0x65, 0x14]);
        // Name 0 = "SIMPLEX".
        img[NAME_BASE..NAME_BASE + 7].copy_from_slice(b"SIMPLEX");
        let ch = decode_channels(&img);
        assert_eq!(ch.len(), 1);
        assert_eq!(ch[0].index, 0);
        assert_eq!(ch[0].name, "SIMPLEX");
        assert!((ch[0].rx_mhz - 146.520).abs() < 1e-6);
    }

    // ---- encoding ----

    #[test]
    fn freq_encodes_to_known_lbcd() {
        assert_eq!(freq_to_lbcd(146.520), [0x00, 0x20, 0x65, 0x14]);
        // Round-trips through the decoder.
        assert!((lbcd_to_u32(&freq_to_lbcd(446.000)) as f64 * 10.0 / 1e6 - 446.0).abs() < 1e-6);
    }

    #[test]
    fn tone_mode_is_tx_ctcss_only() {
        let c = Channel {
            tone_mode: Some("Tone".into()),
            ctcss_uplink: Some(100.0),
            ctcss_downlink: Some(88.5),
            ..Default::default()
        };
        // Tone: transmit uplink 100.0 (=1000), RX open.
        assert_eq!(encode_tones(&c), (0, 1000));
    }

    #[test]
    fn tsql_sets_both_sides_to_downlink() {
        let c = Channel {
            tone_mode: Some("TSQL".into()),
            ctcss_uplink: Some(100.0),
            ctcss_downlink: Some(88.5),
            ..Default::default()
        };
        // TSQL: both rx and tx use the downlink (ctone) 88.5 (=885).
        assert_eq!(encode_tones(&c), (885, 885));
    }

    #[test]
    fn dtcs_maps_to_code_index_with_polarity() {
        // 023 is the first code in UV5R_DTCS -> index 0 -> stored 1.
        let normal = Channel {
            tone_mode: Some("DTCS".into()),
            dcs_code: Some("023".into()),
            dcs_polarity: "NN".into(),
            ..Default::default()
        };
        assert_eq!(encode_tones(&normal), (1, 1));
        // Reverse polarity adds 0x69 (-> 0x6A) on the reversed side(s).
        let tx_rev = Channel {
            dcs_polarity: "RN".into(),
            ..normal.clone()
        };
        assert_eq!(encode_tones(&tx_rev), (1, 0x6A)); // (rx normal, tx reverse)
        let both_rev = Channel {
            dcs_polarity: "RR".into(),
            ..normal.clone()
        };
        assert_eq!(encode_tones(&both_rev), (0x6A, 0x6A));
    }

    #[test]
    fn cross_mode_picks_per_side_tone_or_dtcs() {
        // DTCS->Tone: TX uses dcs_code (023 -> 1), RX uses downlink CTCSS.
        let c = Channel {
            tone_mode: Some("Cross".into()),
            cross_mode: "DTCS->Tone".into(),
            dcs_code: Some("023".into()),
            dcs_rx_code: Some("025".into()),
            ctcss_uplink: Some(100.0),
            ctcss_downlink: Some(88.5),
            dcs_polarity: "NN".into(),
            ..Default::default()
        };
        // returns (rxtone, txtone): rx = 885 (downlink CTCSS), tx = 1 (DTCS 023).
        assert_eq!(encode_tones(&c), (885, 1));
    }

    #[test]
    fn power_low_sets_lowpower_bit() {
        let high = Channel {
            power: Some("High".into()),
            ..Default::default()
        };
        let low = Channel {
            power: Some("Low".into()),
            ..Default::default()
        };
        let unset = Channel::default();
        assert_eq!(encode_power(&high), 0);
        assert_eq!(encode_power(&low), 1);
        assert_eq!(encode_power(&unset), 0); // default -> High/max
        // "Med" (4W) maps to the UV-5R's High (4W).
        let med = Channel {
            power: Some("Med".into()),
            ..Default::default()
        };
        assert_eq!(encode_power(&med), 0);
    }

    #[test]
    fn tone_field_encode_decode_round_trip() {
        // Each stored value should decode back to a recognizable label.
        assert_eq!(decode_tone_field(0), "");
        assert_eq!(decode_tone_field(885), "88.5"); // CTCSS
        assert_eq!(decode_tone_field(1), "D023 N"); // DTCS normal, first code
        assert_eq!(decode_tone_field(0x6A), "D023 R"); // DTCS reverse, first code
        // Summary reconstruction of the universal modes.
        assert_eq!(decode_tone_summary(0, 1000), "T 100.0");
        assert_eq!(decode_tone_summary(885, 885), "TSQL 88.5");
        assert_eq!(decode_tone_summary(1, 1), "DTCS 023 N");
        assert_eq!(decode_tone_summary(885, 1), "Cross D023 N->88.5");
    }

    #[test]
    fn channel_encodes_freq_offset_and_flags() {
        // 145.310 MHz, -0.6 offset, NFM -> tx 144.710, narrow, scannable.
        let c = Channel {
            rx_freq: 145.310,
            duplex: Some("-".into()),
            offset: Some(0.6),
            mode: Some("NFM".into()),
            ..Default::default()
        };
        let m = encode_channel(&c);
        assert_eq!(&m[0..4], &freq_to_lbcd(145.310));
        assert_eq!(&m[4..8], &freq_to_lbcd(144.710));
        // byte15: wide(bit6)=0 for NFM, scan(bit2)=1 -> 0x04.
        assert_eq!(m[15], 0x04);
        // FM would set the wide bit.
        let fm = Channel {
            mode: Some("FM".into()),
            ..c.clone()
        };
        assert_eq!(encode_channel(&fm)[15], 0x44);
    }

    #[test]
    fn patch_writes_rows_and_clears_the_rest() {
        let mut img = vec![0x00u8; 0x1808];
        let slots = vec![SlotChannel {
            slot: 0,
            name: "SIMPLEX".to_string(),
            channel: Channel {
                rx_freq: 146.520,
                mode: Some("FM".into()),
                ..Default::default()
            },
        }];
        patch_image(&mut img, &slots);
        let decoded = decode_channels(&img);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].name, "SIMPLEX");
        assert!((decoded[0].rx_mhz - 146.520).abs() < 1e-6);
        // Slot 1 was cleared to 0xFF (empty).
        assert!(img[CHANNEL_BASE + ENTRY_LEN..CHANNEL_BASE + 2 * ENTRY_LEN]
            .iter()
            .all(|&b| b == 0xFF));
    }

    #[test]
    fn patch_honors_slot_indices_and_clears_unassigned() {
        // patch_image writes each channel to its given slot and clears any slot
        // not assigned. (The resolver packs slots contiguously, but patch_image
        // must place channels by index and blank the rest regardless.)
        let mut img = vec![0x00u8; 0x1808];
        let slots = vec![
            SlotChannel {
                slot: 0,
                name: "A".into(),
                channel: Channel {
                    rx_freq: 146.520,
                    mode: Some("FM".into()),
                    ..Default::default()
                },
            },
            SlotChannel {
                slot: 2,
                name: "C".into(),
                channel: Channel {
                    rx_freq: 446.000,
                    mode: Some("FM".into()),
                    ..Default::default()
                },
            },
        ];
        patch_image(&mut img, &slots);
        let decoded = decode_channels(&img);
        assert_eq!(decoded.len(), 2);
        // Positions are preserved: A at memory 0, C at memory 2 (not compacted).
        assert_eq!(decoded[0].index, 0);
        assert_eq!(decoded[0].name, "A");
        assert_eq!(decoded[1].index, 2);
        assert_eq!(decoded[1].name, "C");
        // The gap at slot 1 is cleared to 0xFF (empty).
        assert!(img[CHANNEL_BASE + ENTRY_LEN..CHANNEL_BASE + 2 * ENTRY_LEN]
            .iter()
            .all(|&b| b == 0xFF));
    }

    #[test]
    fn names_uppercase_and_pad() {
        assert_eq!(&name_bytes("Simplex"), b"SIMPLEX");
        assert_eq!(name_bytes("CO")[2..], [0xFF; 5]);
    }
}
