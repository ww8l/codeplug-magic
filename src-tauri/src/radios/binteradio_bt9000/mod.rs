//! Binteradio BT-9000 (issue #43) — clone-mode cable radio.
//!
//! ## What this radio is
//!
//! One badge on an OEM platform sold as the Radtel RT-950 Pro, Bajeton BJ-9000
//! and Tenway TP-900 Pro. The platform's own name is in the protocol: the clone
//! session opens with the ASCII string `PROGRAMBT9000U`. The radio nonetheless
//! reports its model as `RT-950`, so [`MODEL_TOKEN`] is what the handshake
//! checks — never the badge on the case.
//!
//! 960 channels in 15 fixed zones of 64. Zones have **no names in the radio**:
//! membership is `index / 64`, and the vendor CPS keeps zone labels only in its
//! own `.dat` file. There is nowhere in the clone image to put them.
//!
//! ## Protocol
//!
//! 115200 8N1. Handshake, then a negotiated 4-byte XOR key obfuscates every
//! 0x80-byte payload. Read `0x52` / write `0x57` over six segments, plus an
//! APRS block reached by `0x54` / `0x55` in its own address space.
//!
//! ## Three things measured on the radio that the published map got wrong
//!
//! 1. **A block ACK can take 15 seconds.** The first write here died at
//!    `0x8080` with a 3 s timeout and nothing wrong with the data — a flash
//!    erase at a segment boundary. See [`ACK_TIMEOUT`].
//! 2. **`0x8080`–`0x80FF` is a firmware journal, not VFO storage.** Writing it
//!    makes the radio append a snapshot of its own VFO state there and discard
//!    ours. [`WRITE_SEGMENTS`] stops the VFO segment at `0x80` for this reason,
//!    and [`assert_writable`] refuses the address outright.
//! 3. **The APRS block is not writable by any sequence found so far.** Its
//!    payload must go unobfuscated to draw any response at all, and even then
//!    the `0x06` it answers is a lie — the block never changes. APRS is
//!    therefore READ-ONLY here; see [`APRS_WRITE_UNPROVEN`].
//!
//! ⚠ **An ACK from this radio does not mean the data landed.** Every claim in
//! this module was verified by reading the image back, never by the ACK.
//!
//! ⚠ **The radio does not validate settings writes.** It stored `127` in four
//! fields whose real maxima are 9, 2, 3 and 1. There is no hardware backstop;
//! every bound has to be enforced here.

pub(crate) mod bt9000_settings_table;
pub(crate) mod dcs;
pub(crate) mod settings;
#[cfg(test)]
mod hw_ladder;

use std::time::Duration;

use serde::Serialize;
use serialport::{ClearBuffer, SerialPort};

use crate::commands::export::SlotChannel;
use crate::models::{Channel, RadioModel};
use crate::radios::driver::{
    CodeplugProgramReport, DecodedChannelSample, ImageProgramRequest, ImageProgrammer,
    ImageRestorer, RadioDriver, RadioIdentity,
};

const BAUD: u32 = 115_200;

/// Ordinary read timeout. Generous next to the other drivers because this radio
/// answers a block header only after it has served the whole 0x80-byte payload.
const TIMEOUT: Duration = Duration::from_secs(3);

/// How long a *write* block ACK may take. Measured, not guessed: at 3 s the
/// identity write died at `0x8080`; at 15 s the identical write completed. The
/// stall is a flash erase at a segment boundary, so it is rare but real, and a
/// driver that gives up early leaves the radio half-programmed.
const ACK_TIMEOUT: Duration = Duration::from_secs(15);

const HANDSHAKE: &[u8] = b"PROGRAMBT9000U";

/// Block commands. Read and write are distinct opcodes in both the main space
/// and the APRS one, and a `Segment` carries whichever its table is for.
const CMD_READ: u8 = 0x52;
const CMD_READ_APRS: u8 = 0x54;
const CMD_WRITE: u8 = 0x57;
const ACK: u8 = 0x06;
const END: u8 = b'E';
const BLOCK: usize = 0x80;

/// What the radio answers to `M`, whatever the badge says. A Binteradio-branded
/// BT-9000 reports `RT-950`.
pub(crate) const MODEL_TOKEN: &str = "RT-950";

/// Exact clone payload length. A longer buffer is refused: streaming a full
/// `0x0000`–`0xFFFF` dump into the clone space is what permanently degraded the
/// transmit path of another radio on this platform.
pub(crate) const IMAGE_LEN: usize = 33_152;

const CHANNEL_COUNT: usize = 960;
const ENTRY_LEN: usize = 32;
const NAME_LEN: usize = 12;
pub(crate) const CHANNELS_PER_ZONE: usize = 64;
pub(crate) const ZONE_COUNT: usize = CHANNEL_COUNT / CHANNELS_PER_ZONE; // 15

/// The 960 memories are 15 fixed zones of 64, with no zone names anywhere in
/// the image. Held as an invariant so a future edit to any one of these three
/// cannot quietly disagree with the other two.
const _: () = assert!(ZONE_COUNT * CHANNELS_PER_ZONE == CHANNEL_COUNT);

// ============================================================
// Segment tables
// ============================================================

/// One contiguous clone region. `name` exists because "channels" and "aprs"
/// both start at address `0x0000` — in different command spaces — so an address
/// alone does not identify a segment.
#[derive(Clone, Copy)]
pub(crate) struct Segment {
    pub name: &'static str,
    pub command: u8,
    /// Address as the radio sees it, inside this command's space.
    pub address: u16,
    /// Offset of this segment's bytes within the assembled image.
    pub file_offset: usize,
    pub length: usize,
}

/// Read layout. Reads are permissive on this radio — it will serve any address
/// to `0x52` — so this table defines the image, not the radio's limits.
pub(crate) const READ_SEGMENTS: [Segment; 7] = [
    Segment { name: "channels",  command: 0x52, address: 0x0000, file_offset: 0x0000, length: 0x7800 },
    Segment { name: "vfo",       command: 0x52, address: 0x8000, file_offset: 0x7800, length: 0x0100 },
    Segment { name: "function",  command: 0x52, address: 0x9000, file_offset: 0x7900, length: 0x0100 },
    Segment { name: "dtmf",      command: 0x52, address: 0xA000, file_offset: 0x7A00, length: 0x0200 },
    Segment { name: "mod_param", command: 0x52, address: 0xB000, file_offset: 0x7C00, length: 0x0200 },
    Segment { name: "mod_names", command: 0x52, address: 0xD000, file_offset: 0x7E00, length: 0x0300 },
    Segment { name: "aprs",      command: 0x54, address: 0x0000, file_offset: 0x8100, length: 0x0080 },
];

/// Write layout. Deliberately NOT the read layout:
///
/// - `vfo` stops at `0x80`. The second block is the firmware journal.
/// - `aprs` is absent. It does not commit, and a control that runs and fails is
///   worse than one that is not offered. Established over two sessions and eight
///   attempts: a plain payload is acknowledged with `0x06` in 0.0 s and changes
///   nothing (waiting 16 s rather than 2 does not help), an obfuscated one draws
///   `0x54` — the APRS *read* opcode — rather than an ACK, the block is still
///   unchanged after a POWER CYCLE, and the space holds only the one 0x80 block
///   so the write is not partial. Everything untried needs guessed command
///   bytes, which is desk work on the vendor CPS rather than radio work.
pub(crate) const WRITE_SEGMENTS: [Segment; 6] = [
    Segment { name: "channels",  command: 0x57, address: 0x0000, file_offset: 0x0000, length: 0x7800 },
    Segment { name: "vfo",       command: 0x57, address: 0x8000, file_offset: 0x7800, length: 0x0080 },
    Segment { name: "function",  command: 0x57, address: 0x9000, file_offset: 0x7900, length: 0x0100 },
    Segment { name: "dtmf",      command: 0x57, address: 0xA000, file_offset: 0x7A00, length: 0x0200 },
    Segment { name: "mod_param", command: 0x57, address: 0xB000, file_offset: 0x7C00, length: 0x0200 },
    Segment { name: "mod_names", command: 0x57, address: 0xD000, file_offset: 0x7E00, length: 0x0300 },
];

/// The bytes of `seg` a read-back may legitimately be compared against.
///
/// ⚠ Two ranges inside [`WRITE_SEGMENTS`] are FIRMWARE-OWNED and move on their
/// own, so comparing them reports a mismatch on a write that landed perfectly:
///
/// - the `vfo` journal, already excluded by that segment stopping at `0x80`;
/// - the back half of `function`, `0x9080`-`0x90FF`, a shadow copy of the live
///   settings (`+0xD0` onward is byte-for-byte the live block).
///
/// This exists as a helper because the first fix for it was applied to ONE of
/// the three comparisons in this driver. The other two — the restore and the
/// codeplug read-back — kept telling the operator their write had not landed,
/// and the restore is the path somebody reaches for when things have already
/// gone wrong.
pub(crate) fn comparable(seg: Segment) -> std::ops::Range<usize> {
    let len = if seg.name == "function" { FUNCTION_LIVE_LEN } else { seg.length };
    seg.file_offset..seg.file_offset + len
}

/// Refuse a segment whose opcode does not belong to the transport being asked
/// to carry it.
///
/// A [`Segment`] carries its own command byte, and [`READ_SEGMENTS`] and
/// [`WRITE_SEGMENTS`] describe the SAME blocks with different ones. Handing a
/// write segment to the reader would put write opcodes on the wire with nothing
/// behind them, on the platform where a desynchronised write stream has already
/// permanently degraded one radio's transmit. Cheap to check, so it is checked
/// rather than left to the caller picking the right constant.
fn check_commands(segments: &[Segment], allowed: &[u8], verb: &str) -> Result<(), String> {
    for seg in segments {
        if !allowed.contains(&seg.command) {
            return Err(format!(
                "internal error: refusing to {verb} segment {} with command 0x{:02X}, \
                 which is not a {verb} command",
                seg.name, seg.command
            ));
        }
    }
    Ok(())
}

/// Address ranges that must never be written in the `0x52`/`0x57` space.
///
/// `0x7800`–`0x7FFF` is the gap the vendor CPS skips. `0x8080`–`0x80FF` is the
/// VFO journal. Both were implicated in the damage report on this platform.
fn forbidden(address: u16) -> bool {
    (0x7800..0x8000).contains(&address) || (0x8080..0x8100).contains(&address)
}

/// Panics if any write segment reaches a forbidden address. Called by a test,
/// so the table cannot drift back to the unsafe layout unnoticed.
#[cfg(test)]
fn assert_writable() {
    for seg in WRITE_SEGMENTS {
        if seg.command != 0x57 {
            continue;
        }
        for off in (0..seg.length).step_by(BLOCK) {
            let addr = seg.address + off as u16;
            assert!(
                !forbidden(addr),
                "write segment {} reaches forbidden address 0x{addr:04X}",
                seg.name
            );
        }
    }
}

// ============================================================
// XOR obfuscation
// ============================================================

/// The radio's 20 keystream symbols. The negotiation frame we send picks one;
/// the radio derives the same choice from the same bytes.
const ENCRYPT_STRINGS: [&[u8; 4]; 20] = [
    b"BHT ", b"CO 7", b"A ES", b" EIY", b"M PQ",
    b"XN Y", b"RVB ", b" HQP", b"W RC", b"MS N",
    b" SAT", b"K DH", b"ZO R", b"C SL", b"6RB ",
    b" JCG", b"PN V", b"J PK", b"EK L", b"I LZ",
];

/// Build the 25-byte `SEND` frame and the key it selects.
///
/// The vendor software randomises this. We do not, deliberately: the radio
/// derives the key from bytes we choose, so a fixed frame is as valid as a
/// random one and makes a session reproducible when something goes wrong. That
/// two *different* keys decoded the same radio image byte-for-byte is what
/// proved this derivation correct in the first place.
fn encryption_frame() -> ([u8; 25], [u8; 4]) {
    let mut frame = [0u8; 25];
    frame[0..4].copy_from_slice(b"SEND");
    // Low nibble 0, high nibble 1 -> the selector lands on frame[5].
    frame[4] = 0x10;
    frame[5] = 1; // table row 1, "CO 7"
    let code = frame[4];
    let idx = if code & 0x20 != 0 {
        (code as usize - 0x20) * 2 + 1
    } else {
        (code as usize - 0x10) * 2
    } + 1;
    let key = *ENCRYPT_STRINGS[frame[4 + idx] as usize];
    (frame, key)
}

/// Apply the keystream. Symmetric — the same call encodes and decodes.
///
/// The skips are the radio's own rule, not an optimisation: a key byte of
/// `0x20`, or a payload byte of `0x00`/`0xFF`/`k`/`k ^ 0xFF`, passes through
/// untouched. Getting this wrong corrupts an image while still round-tripping
/// against itself, which is why it was checked against two different keys.
fn apply_xor(payload: &mut [u8], key: &[u8; 4]) {
    for (i, value) in payload.iter_mut().enumerate() {
        let k = key[i % 4];
        if k != 0x20 && *value != 0x00 && *value != 0xFF && *value != k && *value != (k ^ 0xFF) {
            *value ^= k;
        }
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

fn read_exact(p: &mut dyn SerialPort, n: usize) -> Result<Vec<u8>, String> {
    let mut buf = vec![0u8; n];
    std::io::Read::read_exact(p, &mut buf)
        .map_err(|e| format!("timed out reading {n} bytes from the radio: {e}"))?;
    Ok(buf)
}

fn write_all(p: &mut dyn SerialPort, data: &[u8]) -> Result<(), String> {
    std::io::Write::write_all(p, data).map_err(|e| format!("serial write failed: {e}"))
}

/// What the radio reports during the clone handshake.
pub(crate) struct Handshake {
    /// The 12-byte model string, trimmed. `RT-950` on every unit seen.
    pub model: String,
    /// The 16-byte blob the `F` probe returns. Its leading bytes read as packed
    /// BCD band edges (`0136 0174 0400 0520 0200 0260 …`), which would make it
    /// the radio's own band table — **unproven**, so it is carried as evidence
    /// rather than parsed.
    pub probe: Vec<u8>,
    key: [u8; 4],
}

/// Open a clone session. Harmless: reads nothing but identity.
pub(crate) fn handshake(p: &mut dyn SerialPort) -> Result<Handshake, String> {
    let _ = p.clear(ClearBuffer::All);

    write_all(p, HANDSHAKE)?;
    if read_exact(p, 1)?[0] != ACK {
        return Err("radio did not acknowledge the clone handshake".into());
    }

    write_all(p, b"F")?;
    let probe = read_exact(p, 16)?;

    write_all(p, b"M")?;
    let raw = read_exact(p, 12)?;
    let model = String::from_utf8_lossy(&raw)
        .trim_matches(|c: char| c == '\0' || c == ' ')
        .to_string();
    if model != MODEL_TOKEN {
        return Err(format!(
            "expected a {MODEL_TOKEN} (the BT-9000 reports that model); radio said {model:?}"
        ));
    }

    let (frame, key) = encryption_frame();
    write_all(p, &frame)?;
    if read_exact(p, 1)?[0] != ACK {
        return Err("radio did not accept the keystream negotiation".into());
    }

    Ok(Handshake { model, probe, key })
}

/// Read the whole clone image. Always exactly [`IMAGE_LEN`] bytes.
pub(crate) fn download(p: &mut dyn SerialPort, hs: &Handshake) -> Result<Vec<u8>, String> {
    download_segments(p, hs, &READ_SEGMENTS)
}

/// Read `segments` only, into an otherwise-zeroed full-length image.
///
/// The buffer stays [`IMAGE_LEN`] so that a caller reading one segment indexes
/// it with the same offsets as one reading the whole radio. Bytes outside
/// `segments` are zero and must not be written back — [`upload_segments`] with
/// the matching segment list is the only safe partner for this.
pub(crate) fn download_segments(
    p: &mut dyn SerialPort,
    hs: &Handshake,
    segments: &[Segment],
) -> Result<Vec<u8>, String> {
    let mut image = vec![0u8; IMAGE_LEN];
    check_commands(segments, &[CMD_READ, CMD_READ_APRS], "read")?;
    for seg in segments.iter().copied() {
        for off in (0..seg.length).step_by(BLOCK) {
            let addr = seg.address + off as u16;
            let header = [seg.command, (addr >> 8) as u8, addr as u8, BLOCK as u8];
            write_all(p, &header)?;
            let reply = read_exact(p, 4 + BLOCK)?;
            let start = seg.file_offset + off;
            let slice = &mut image[start..start + BLOCK];
            slice.copy_from_slice(&reply[4..]);
            apply_xor(slice, &hs.key);
        }
    }
    write_all(p, &[END])?;
    Ok(image)
}

/// The function-configuration segment, on its own.
///
/// A settings write must not go out through the whole-image [`upload`]: that
/// rewrites all 960 channel records to change a squelch level, taking four
/// minutes instead of a third of a second and putting the operator's memories
/// at risk for a change that never touched them. Narrowing a transport that has
/// the reach to write everything is a deliberate act, not an optimisation.
pub(crate) const SETTINGS_SEGMENTS: [Segment; 1] = [WRITE_SEGMENTS[2]];

/// The same block, for READING. Deliberately a separate constant drawn from
/// [`READ_SEGMENTS`]: a `Segment` carries its command byte, and this radio's
/// read and write commands are different (`0x52` vs `0x57`). Handing the write
/// segment to [`download_segments`] would put a WRITE opcode on the wire with
/// no payload behind it — on the platform that has already had a radio's
/// transmit permanently degraded by a desynchronised write stream.
pub(crate) const SETTINGS_READ_SEGMENTS: [Segment; 1] = [READ_SEGMENTS[2]];

/// Where the function block sits in the assembled image, and how long it is.
pub(crate) const FUNCTION_OFFSET: usize = 0x7900;
pub(crate) const FUNCTION_LEN: usize = 0x0100;

/// How much of the function block is LIVE settings.
///
/// ⚠ Measured on the radio (s128), and the rest of the segment is not padding:
///
/// | range | what |
/// |---|---|
/// | `0x00-0x45` | the settings the menus edit |
/// | `0x46-0x7F` | `0xFF` filler |
/// | `0x80-0xFF` | a **firmware-maintained SHADOW** — `+0xD0` onward is byte-for-byte the live block |
///
/// The shadow moves on its own, exactly like the VFO journal at radio `0x8080`.
/// So a settings write can only be VERIFIED across the live area; comparing the
/// whole segment reports a mismatch on bytes the radio owns and we never set.
pub(crate) const FUNCTION_LIVE_LEN: usize = 0x46;

/// Write an image back. Only [`WRITE_SEGMENTS`] is addressed, so the CPS gap,
/// the VFO journal and the APRS block are never touched.
///
/// Aborts on the first block the radio does not acknowledge. That is not
/// caution for its own sake: streaming past a missing ACK desynchronises the
/// radio's write pointer, and doing so is what damaged a radio on this platform.
pub(crate) fn upload(p: &mut dyn SerialPort, hs: &Handshake, image: &[u8]) -> Result<(), String> {
    upload_segments(p, hs, image, &WRITE_SEGMENTS)
}

/// Write `segments` of `image` back, and nothing else.
///
/// `segments` must be drawn from [`WRITE_SEGMENTS`] — the address guard below
/// is the backstop, not the policy. The whole image is still required as the
/// argument so that every offset means the same thing everywhere in this
/// driver, and so a caller cannot hand over a buffer that has been shifted.
pub(crate) fn upload_segments(
    p: &mut dyn SerialPort,
    hs: &Handshake,
    image: &[u8],
    segments: &[Segment],
) -> Result<(), String> {
    if image.len() != IMAGE_LEN {
        return Err(format!(
            "refusing to write a {}-byte image; a BT-9000 clone is exactly {IMAGE_LEN} bytes",
            image.len()
        ));
    }
    p.set_timeout(ACK_TIMEOUT)
        .map_err(|e| format!("could not extend the serial timeout for writing: {e}"))?;

    check_commands(segments, &[CMD_WRITE], "write")?;
    for seg in segments.iter().copied() {
        for off in (0..seg.length).step_by(BLOCK) {
            let addr = seg.address + off as u16;
            if forbidden(addr) {
                return Err(format!(
                    "internal error: refusing to write 0x{addr:04X}, a firmware-managed address"
                ));
            }
            let start = seg.file_offset + off;
            let mut payload = image[start..start + BLOCK].to_vec();
            apply_xor(&mut payload, &hs.key);
            let header = [seg.command, (addr >> 8) as u8, addr as u8, BLOCK as u8];
            write_all(p, &header)?;
            write_all(p, &payload)?;
            let ack = read_exact(p, 1)?[0];
            if ack != ACK {
                return Err(format!(
                    "radio rejected the block at 0x{addr:04X} (answered 0x{ack:02X}); \
                     write stopped there"
                ));
            }
        }
    }
    write_all(p, &[END])?;
    let _ = p.set_timeout(TIMEOUT);
    Ok(())
}

// ============================================================
// Container
// ============================================================

/// Reject anything that is not a BT-9000 clone image before a byte of it
/// reaches the radio.
pub(crate) fn validate_image(image: &[u8]) -> Result<(), String> {
    if image.len() != IMAGE_LEN {
        return Err(format!(
            "not a BT-9000 image: {} bytes, expected exactly {IMAGE_LEN}",
            image.len()
        ));
    }
    Ok(())
}

// ============================================================
// Channel records
// ============================================================

#[derive(Serialize, PartialEq, Debug, Clone)]
pub struct Bt9000DecodedChannel {
    pub index: usize,
    /// 1-based zone, `index / 64 + 1`. The radio stores no zone names.
    pub zone: usize,
    pub name: String,
    pub rx_mhz: f64,
    pub tx_mhz: f64,
    pub rx_tone: String,
    pub tx_tone: String,
    pub power: String,
    pub narrow: bool,
    pub tx_enabled: bool,
}

/// `0 = High, 1 = Middle, 2 = Low` — confirmed on the radio's own screen.
/// Not the order the manual prints them in, which is the usual trap.
const POWER_LEVELS: [&str; 3] = ["High", "Middle", "Low"];

/// Decode a frequency: packed BCD, **least-significant byte first**, in units of
/// 10 Hz. `00 00 51 14` is 145.100 MHz. The published note reads these
/// big-endian, which is wrong for this radio.
fn lbcd_to_hz(b: &[u8]) -> u64 {
    let mut v: u64 = 0;
    for byte in b.iter().rev() {
        v = v * 100 + u64::from((byte >> 4) * 10 + (byte & 0x0F));
    }
    v * 10
}

fn hz_to_lbcd(hz: u64) -> [u8; 4] {
    let mut units = hz / 10;
    let mut out = [0u8; 4];
    for slot in out.iter_mut() {
        let pair = (units % 100) as u8;
        *slot = ((pair / 10) << 4) | (pair % 10);
        units /= 100;
    }
    out
}

/// Decode a two-byte tone field.
///
/// `00 00` is off. A zero *second* byte means DCS, and the first byte is a
/// 1-based index into [`dcs::DCS_TABLE`]. Otherwise it is a little-endian u16
/// of Hz×10. The two cannot collide: the lowest CTCSS tone, 67.0 Hz, is 670,
/// whose high byte is already non-zero.
fn decode_tone(raw: &[u8]) -> String {
    match (raw[0], raw[1]) {
        (0, 0) => "—".to_string(),
        (idx, 0) => match dcs::byte_to_dcs(idx) {
            Some((code, inverted)) => format!("DTCS {code:03} {}", if inverted { "I" } else { "N" }),
            None => "—".to_string(),
        },
        (lo, hi) => {
            let value = u16::from(lo) | (u16::from(hi) << 8);
            if value == 0xFFFF {
                "—".to_string()
            } else {
                format!("T {:.1}", f64::from(value) / 10.0)
            }
        }
    }
}

fn encode_ctcss(hz: f64) -> [u8; 2] {
    let v = (hz * 10.0).round() as u16;
    [v as u8, (v >> 8) as u8]
}

fn encode_dcs(code: &str, inverted: bool) -> Option<[u8; 2]> {
    let numeric: u16 = code.trim().parse().ok()?;
    dcs::dcs_to_byte(numeric, inverted).map(|b| [b, 0x00])
}

pub(crate) fn decode_channels(image: &[u8]) -> Vec<Bt9000DecodedChannel> {
    let mut out = Vec::new();
    for i in 0..CHANNEL_COUNT {
        let rec = &image[i * ENTRY_LEN..(i + 1) * ENTRY_LEN];
        // An empty slot reads as all-0xFF in its frequency field.
        if rec[0..4] == [0xFF; 4] {
            continue;
        }
        let rx = lbcd_to_hz(&rec[0..4]);
        if rx == 0 {
            continue;
        }
        let flags = rec[15];
        out.push(Bt9000DecodedChannel {
            index: i,
            zone: i / CHANNELS_PER_ZONE + 1,
            name: decode_name(&rec[20..32]),
            rx_mhz: rx as f64 / 1e6,
            tx_mhz: lbcd_to_hz(&rec[4..8]) as f64 / 1e6,
            rx_tone: decode_tone(&rec[8..10]),
            tx_tone: decode_tone(&rec[10..12]),
            power: POWER_LEVELS[usize::from(rec[14] & 0x0F).min(2)].to_string(),
            narrow: flags & 0x40 != 0,
            tx_enabled: flags & 0x02 != 0,
        });
    }
    out
}

/// Names are plain ASCII. **Two sentinels, not one**: a channel that has never
/// been named is twelve `0x00` bytes, while a named channel is padded with
/// `0xFF`. The published note mentions only `0xFF`, and an encoder that pads a
/// blank channel with it is not reproducing what the radio writes.
fn decode_name(raw: &[u8]) -> String {
    raw.iter()
        .take_while(|&&b| b != 0xFF && b != 0x00)
        .map(|&b| b as char)
        .collect::<String>()
        .trim_end()
        .to_string()
}

fn name_bytes(name: &str) -> [u8; NAME_LEN] {
    let mut out = [0xFFu8; NAME_LEN];
    if name.is_empty() {
        return [0x00; NAME_LEN];
    }
    for (slot, ch) in out.iter_mut().zip(name.chars().take(NAME_LEN)) {
        *slot = if ch.is_ascii() && !ch.is_control() { ch as u8 } else { b' ' };
    }
    out
}

/// `0 = High, 1 = Middle, 2 = Low`, confirmed on the radio's screen for all
/// three. Note this is the reverse of the TD-H3's mapping in this same crate —
/// the reason each driver measures its own rather than sharing a helper.
fn power_index(c: &Channel) -> u8 {
    match c.power.as_deref() {
        Some(p) if p.eq_ignore_ascii_case("Low") => 2,
        Some(p)
            if p.eq_ignore_ascii_case("Med")
                || p.eq_ignore_ascii_case("Medium")
                || p.eq_ignore_ascii_case("Mid")
                || p.eq_ignore_ascii_case("Middle") =>
        {
            1
        }
        _ => 0,
    }
}

fn tone_off() -> [u8; 2] {
    [0x00, 0x00]
}

/// DCS for one direction. An unknown code falls back to no tone rather than to
/// a neighbouring table entry: a silently *different* valid tone is worse on a
/// repeater than no tone at all.
fn tone_dtcs(code: &Option<String>, inverted: bool) -> [u8; 2] {
    code.as_deref()
        .and_then(|c| encode_dcs(c, inverted))
        .unwrap_or_else(tone_off)
}

/// Same tone-mode vocabulary the other drivers use, so a channel means the same
/// thing on every radio in the library. Returns `(rx, tx)`.
fn encode_tones(c: &Channel) -> ([u8; 2], [u8; 2]) {
    let pol = c.dcs_polarity.as_bytes();
    let tx_rev = pol.first() == Some(&b'R');
    let rx_rev = pol.get(1) == Some(&b'R');
    let ctcss = |t: Option<f64>| t.map(encode_ctcss).unwrap_or_else(tone_off);

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

/// Build one 32-byte channel record.
///
/// The TX shift is carried entirely by the stored TX frequency — there is no
/// separate direction field, confirmed against the radio (a −0.600 repeater
/// channel differs from a simplex one only in bytes 4-7).
/// `tx_enable` clears byte 15 bit 1 for a receive-only memory.
///
/// ⚠ That bit is **measured**, not inherited: a channel written with it clear
/// refuses the PTT while its neighbour with the bit set keys normally, checked
/// on the radio (s128). It was a source claim until then, which is why this
/// driver spent a release setting it unconditionally and guarding the gap with
/// a test instead of guessing.
fn encode_channel(c: &Channel, name: &str, tx_hz: u64, tx_enable: bool) -> [u8; ENTRY_LEN] {
    let mut m = [0u8; ENTRY_LEN];

    m[0..4].copy_from_slice(&hz_to_lbcd((c.rx_freq * 1e6).round() as u64));
    m[4..8].copy_from_slice(&hz_to_lbcd(tx_hz));

    let (rx_tone, tx_tone) = encode_tones(c);
    m[8..10].copy_from_slice(&rx_tone);
    m[10..12].copy_from_slice(&tx_tone);

    // 12 = signalling group, 13 = PTT-ID. Both left at "none": neither is a
    // channel property this app models, and the radio's own default is 0.
    m[14] = power_index(c) & 0x0F; // high nibble is the scrambler, left off

    // bit 1 = TX enable, bit 6 = narrow. Everything else (FHSS, encryption,
    // busy lockout, scan-add, AM) stays off — measured defaults, not guesses.
    //
    // ⚠ Narrow ONLY on an explicit narrow mode. `mode` is nullable in the
    // schema and reachable from a CSV import with no mode column, and
    // `export::channel_fit` resolves a NULL to "FM" when it decides the channel
    // is programmable — so treating NULL as *not* FM narrowed a channel that
    // the fit logic had just admitted as wide FM. The two now agree.
    //
    // ⚠ AM is a known gap, deliberately left. This radio receives AM and byte
    // 15 bit 0 is claimed to select it, but that bit has never been measured
    // here, and an AM channel inside 136-174 MHz therefore goes out as wide FM.
    // `scratchpad/binteradio_bt9000/SCREEN-CHECK.md` carries the measurement;
    // guessing the bit is how radios get damaged on this platform.
    let narrow = matches!(c.mode.as_deref(), Some(m) if m.eq_ignore_ascii_case("NFM"));
    m[15] = if tx_enable { 0x02 } else { 0x00 } | if narrow { 0x40 } else { 0x00 };

    // 16-19 = FHSS code, left zero.
    m[20..32].copy_from_slice(&name_bytes(name));
    m
}

/// Patch resolved channel slots into a freshly-read image.
///
/// Only the channel segment is touched. Slots the codeplug does not fill are
/// **cleared to the radio's own empty form** rather than left alone, so
/// programming a shorter codeplug does not leave stale channels behind.
pub(crate) fn patch_image(image: &mut [u8], slots: &[SlotChannel], model: &RadioModel) {
    for i in 0..CHANNEL_COUNT {
        image[i * ENTRY_LEN..(i + 1) * ENTRY_LEN].fill(0xFF);
    }
    for s in slots {
        if s.slot >= CHANNEL_COUNT {
            continue;
        }
        let tx_hz = (crate::commands::export::tx_frequency(&s.channel) * 1e6).round() as u64;
        // A channel the radio can hear but not transmit on is programmed with
        // the PTT disabled rather than dropped — and rather than left
        // transmit-enabled, which on a radio that validates nothing would hand
        // the operator a memory that keys up out of band.
        let tx_enable = !matches!(
            crate::commands::export::channel_fit(&s.channel, model),
            crate::commands::export::ChannelFit::ReceiveOnly(_)
        );
        let rec = encode_channel(&s.channel, &s.name, tx_hz, tx_enable);
        image[s.slot * ENTRY_LEN..(s.slot + 1) * ENTRY_LEN].copy_from_slice(&rec);
    }
}


// ============================================================
// Driver
// ============================================================

pub(crate) struct BinteradioBt9000;

/// Registry entry (see `radios/registry.rs`).
pub(crate) static DRIVER: BinteradioBt9000 = BinteradioBt9000;

impl RadioDriver for BinteradioBt9000 {
    fn key(&self) -> &'static str {
        "binteradio_bt9000"
    }

    fn display_name(&self) -> &'static str {
        "Binteradio BT-9000"
    }

    fn baud(&self) -> u32 {
        BAUD
    }

    fn identify(&self, port: &str) -> Result<RadioIdentity, String> {
        let mut p = open_port(port)?;
        let hs = handshake(&mut *p)?;
        // Leave the session cleanly. An aborted clone session leaves bytes in
        // the radio's buffer that surface as bogus answers to the NEXT command
        // — two early reads here returned 0x54 and 0x52 for exactly that
        // reason, and were briefly mistaken for protocol findings.
        let _ = write_all(&mut *p, &[END]);
        Ok(RadioIdentity {
            matched: hs.model.clone(),
            ident_hex: hex(&hs.probe),
            ident_ascii: Some(hs.model),
        })
    }

    fn as_image_programmer(&self) -> Option<&dyn ImageProgrammer> {
        Some(self)
    }

    fn as_image_restorer(&self) -> Option<&dyn ImageRestorer> {
        Some(self)
    }

    fn as_settings_reader(&self) -> Option<&dyn crate::radios::driver::SettingsReader> {
        Some(self)
    }

    fn as_settings_writer(&self) -> Option<&dyn crate::radios::driver::SettingsWriter> {
        Some(self)
    }
}

/// Putting a backup back on the radio.
///
/// Offered because this driver's own error messages promise it. Every failure
/// path in here hands the operator a pre-write backup and tells them it can go
/// back over the same cable — and until this existed, no control in the app
/// could do that, on the one radio in this crate whose platform has a
/// documented unit with permanently degraded transmit.
///
/// It is not new risk: hardware ladder step 1 was exactly this operation — a
/// byte-identical image written back and read back — and it passed.
impl ImageRestorer for BinteradioBt9000 {
    /// Refuse a file that is not a BT-9000 clone image before a byte of it
    /// reaches the radio.
    ///
    /// Length is what separates the formats in `radio-backups/`, which holds
    /// images for every radio this app talks to and is where the picker opens.
    /// 33,152 bytes is this radio's exact clone payload and is shared with none
    /// of the others.
    ///
    /// ⚠ The check is deliberately shape-only, and does NOT try to prove which
    /// unit the image came from. Restoring a backup taken from another radio of
    /// the same model is a normal thing to do, and this is the path reached for
    /// after a bad write. There is also nothing in the image to key on: it
    /// carries no serial number, no ident prefix and — measured in s127 — no
    /// checksum anywhere in its 33,152 bytes.
    fn check_restore_image(&self, image: &[u8]) -> Result<(), String> {
        if image.len() != IMAGE_LEN {
            return Err(format!(
                "this file is {} bytes — a BT-9000 backup is exactly {IMAGE_LEN}. Pick a \
                 .img taken from a BT-9000 (radio-backups/ also holds images for other \
                 radios, which must not be written to this one).",
                image.len()
            ));
        }
        Ok(())
    }

    /// Write the whole backup back in one session, then read it back.
    ///
    /// ⚠ The read-back is not decoration on this radio: it acknowledges blocks
    /// it does not always commit, so an all-ACKs write is not evidence. A
    /// restore that cannot be confirmed says so rather than reporting success —
    /// this is the path somebody reaches for when a write has already gone
    /// wrong, and it is the worst possible place to be optimistic.
    fn restore_image(&self, port: &str, image: &[u8]) -> Result<(), String> {
        self.check_restore_image(image)?;
        let mut p = open_port(port)?;
        let hs = handshake(&mut *p)?;
        upload(&mut *p, &hs, image)?;

        std::thread::sleep(SETTLE);
        let hs = handshake(&mut *p)?;
        let back = download(&mut *p, &hs)?;
        for seg in WRITE_SEGMENTS {
            let r = comparable(seg);
            if image[r.clone()] != back[r] {
                return Err(format!(
                    "the radio acknowledged the restore but segment {} read back \
                     differently. The radio does NOT hold this backup. Power-cycle it \
                     and try the restore again.",
                    seg.name
                ));
            }
        }
        Ok(())
    }
}

impl ImageProgrammer for BinteradioBt9000 {
    /// Yes. A codeplug program writes the profile's settings alongside the
    /// channels.
    ///
    /// This read `false` until it was measured what the whole-image [`upload`]
    /// already does: it addresses [`WRITE_SEGMENTS`], and the function block is
    /// one of them — so a channel program was ALREADY rewriting the settings
    /// segment, just with the bytes it had read a moment earlier. Nothing about
    /// the write got wider here; what changed is that the profile's values go
    /// into that segment before it goes out, instead of the radio's own.
    ///
    /// The old note said settings were held back because this radio validates
    /// nothing and an unintended value would be stored rather than rejected.
    /// That risk is bounded by [`settings::apply_profile_settings`] being a
    /// PATCH: a key the profile does not carry is left exactly as the radio had
    /// it, and the command layer only fills `req.settings` from a profile the
    /// operator saved. The standalone `write_settings` path already accepted
    /// the same values on the same encoder.
    fn carries_profile_settings(&self) -> bool {
        true
    }

    fn download_image(&self, port: &str) -> Result<(RadioIdentity, Vec<u8>), String> {
        let mut p = open_port(port)?;
        let hs = handshake(&mut *p)?;
        let image = download(&mut *p, &hs)?;
        Ok((
            RadioIdentity {
                matched: hs.model.clone(),
                ident_hex: hex(&hs.probe),
                ident_ascii: Some(hs.model),
            },
            image,
        ))
    }

    fn decode_sample(&self, image: &[u8]) -> Vec<DecodedChannelSample> {
        decode_channels(image).into_iter().map(decoded_to_sample).collect()
    }

    fn upload_image(&self, port: &str, image: &[u8]) -> Result<(), String> {
        validate_image(image)?;
        let mut p = open_port(port)?;
        let hs = handshake(&mut *p)?;
        upload(&mut *p, &hs, image)
    }

    fn build_image(
        &self,
        model: &RadioModel,
        channels: &[SlotChannel],
        base: &[u8],
    ) -> Result<Vec<u8>, String> {
        validate_image(base)?;
        if channels.len() > CHANNEL_COUNT {
            return Err(format!(
                "{} channels exceed the BT-9000's {CHANNEL_COUNT} memories.",
                channels.len()
            ));
        }
        let mut image = base.to_vec();
        patch_image(&mut image, channels, model);
        Ok(image)
    }

    /// Download + back up, patch channels into that image, write it back, read
    /// back and verify.
    ///
    /// `req.settings` is patched into the function block on the way out, so a
    /// program leaves the radio holding the profile in full — channels AND
    /// settings. Only the keys the profile carries move; every other byte of
    /// that block goes back exactly as it was read.
    fn program_codeplug(
        &self,
        port: &str,
        req: &ImageProgramRequest,
    ) -> Result<CodeplugProgramReport, String> {
        if req.channels.len() > CHANNEL_COUNT {
            return Err(format!(
                "Codeplug has {} programmable channels, but the BT-9000 holds only {CHANNEL_COUNT}.",
                req.channels.len()
            ));
        }
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let slug = slug_label(req.label);
        let backup_path = req.backup_dir.join(if slug.is_empty() {
            format!("bt9000-prewrite-{stamp}.img")
        } else {
            format!("bt9000-prewrite-{slug}-{stamp}.img")
        });

        let mut p = open_port(port)?;

        // 1. Download + back up.
        let hs = handshake(&mut *p)?;
        let mut image = download(&mut *p, &hs)?;
        std::fs::write(&backup_path, &image)
            .map_err(|e| format!("could not write backup {}: {e}", backup_path.display()))?;

        // 2. Patch channels into the image we just read, so every byte we do
        //    not own goes back exactly as it came.
        let channels_written = req.channels.len();
        patch_image(&mut image, req.channels, req.model);

        // 2b. Patch the profile's settings into the function block, if the
        //     profile carries any. Before the write, so a value this driver's
        //     encoder refuses aborts with nothing on the wire. The range strip
        //     runs first for the same reason it does in `write_settings`: a
        //     stale profile value is dropped with a note rather than failing
        //     the whole program. (The command layer strips too; this covers the
        //     hardware-ladder callers, which reach the trait directly.)
        let mut warnings: Vec<String> = Vec::new();
        let settings_written = match req.settings {
            Some((settings, schema)) => {
                let mut settings = settings.clone();
                warnings.extend(crate::radios::settings_bounds::strip_out_of_range(
                    schema,
                    &mut settings,
                ));
                let (written, skipped) =
                    settings::apply_profile_settings(&mut image, &settings)?;
                warnings.extend(skipped);
                Some(written)
            }
            None => None,
        };

        let restore_hint = |e: String| {
            crate::radios::driver::with_restore_hint(
                e,
                &backup_path,
                "Keep that file. It is the only copy of what was on the radio before \
                 this write, and it can be uploaded back over the same cable.",
            )
        };

        // 3. Write. A fresh session: the radio needs a moment to settle after a
        //    full read before it will answer again.
        std::thread::sleep(SETTLE);
        let hs = handshake(&mut *p).map_err(|e| restore_hint(e.to_string()))?;
        upload(&mut *p, &hs, &image).map_err(restore_hint)?;

        // 4. Read back and verify. Non-fatal: every block was acknowledged.
        //    ⚠ But an ACK on this radio does not prove a commit, so the
        //    read-back is the only real evidence and its absence is reported.
        std::thread::sleep(SETTLE);
        let (verified, note) = match verify_after_write(&mut *p, &image) {
            Ok(result) => result,
            Err(e) => (
                false,
                Some(format!(
                    "Write completed, but read-back verification could not run ({e}). \
                     This radio acknowledges blocks it does not always commit, so \
                     power-cycle it and use Download to confirm before trusting it."
                )),
            ),
        };

        Ok(CodeplugProgramReport {
            channels_written,
            slots_cleared: CHANNEL_COUNT - channels_written,
            settings_written,
            verified: Some(verified),
            note,
            backup_path: backup_path.to_string_lossy().to_string(),
            channels: decode_channels(&image).into_iter().map(decoded_to_sample).collect(),
            zones_written: 0,
            zones_cleared: 0,
            scan_lists_written: 0,
            scan_lists_cleared: 0,
            contacts_written: 0,
            contacts_cleared: 0,
            expected_path: None,
            windows_written: Vec::new(),
            skipped: Vec::new(),
            warnings,
        })
    }
}

/// How long the radio needs between a completed session and the next one. A
/// read issued immediately after a write times out; measured at three seconds,
/// given headroom here.
const SETTLE: Duration = Duration::from_secs(5);

/// Read the image back and compare only the regions we actually wrote.
///
/// The VFO journal, the function block's shadow half and the APRS block are all
/// excluded because the radio owns them: comparing them would report a
/// difference on every single write. See [`comparable`].
fn verify_after_write(
    p: &mut dyn SerialPort,
    expected: &[u8],
) -> Result<(bool, Option<String>), String> {
    let hs = handshake(p)?;
    let actual = download(p, &hs)?;
    let mut mismatched = Vec::new();
    for seg in WRITE_SEGMENTS {
        let range = comparable(seg);
        if expected[range.clone()] != actual[range] {
            mismatched.push(seg.name);
        }
    }
    if mismatched.is_empty() {
        Ok((true, None))
    } else {
        Ok((
            false,
            Some(format!(
                "Read-back does not match what was written ({}). The radio \
                 acknowledged every block, which on this model is not proof of a \
                 commit — restore from the backup and try again.",
                mismatched.join(", ")
            )),
        ))
    }
}

fn decoded_to_sample(c: Bt9000DecodedChannel) -> DecodedChannelSample {
    DecodedChannelSample {
        index: c.index,
        name: c.name,
        rx_mhz: c.rx_mhz,
        shift: Some(if !c.tx_enabled {
            "RX-only".to_string()
        } else if (c.tx_mhz - c.rx_mhz).abs() < 1e-9 {
            String::new()
        } else {
            format!("{:+.3}", c.tx_mhz - c.rx_mhz)
        }),
        tone: c.rx_tone,
        power: c.power,
        mode: Some(if c.narrow { "NFM".into() } else { "FM".into() }),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// Filesystem-safe slug for a codeplug label, so several codeplugs for one
/// radio stay distinguishable among the backups.
fn slug_label(label: &str) -> String {
    label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two channel records taken verbatim off Tim's radio on 2026-09-01, plus
    /// the three the encoding probe wrote and the radio confirmed on its own
    /// screen. These are the anchor: everything else in this module is checked
    /// against bytes the radio actually authored.
    const RADIO_CH1: [u8; 32] = [
        0x00, 0x00, 0x51, 0x14, 0x00, 0x00, 0x51, 0x14, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x50, 0x4C, 0x55, 0x47,
        0x37, 0x33, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    ];
    /// TONEA: 146.520 simplex, CTCSS 88.5 both, High, Wide.
    const PROBE_TONEA: [u8; 32] = [
        0x00, 0x20, 0x65, 0x14, 0x00, 0x20, 0x65, 0x14, 0x75, 0x03, 0x75, 0x03,
        0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x54, 0x4F, 0x4E, 0x45,
        0x41, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    ];
    /// TONEB: 146.940 / −0.600, DCS 023N both, Low, Narrow.
    const PROBE_TONEB: [u8; 32] = [
        0x00, 0x40, 0x69, 0x14, 0x00, 0x40, 0x63, 0x14, 0x01, 0x00, 0x01, 0x00,
        0x00, 0x00, 0x02, 0x42, 0x00, 0x00, 0x00, 0x00, 0x54, 0x4F, 0x4E, 0x45,
        0x42, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    ];
    /// TONEC: 442.000 simplex, RX CTCSS 141.3 / TX DCS 754I, Middle, Wide.
    const PROBE_TONEC: [u8; 32] = [
        0x00, 0x00, 0x20, 0x44, 0x00, 0x00, 0x20, 0x44, 0x85, 0x05, 0xD2, 0x00,
        0x00, 0x00, 0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x54, 0x4F, 0x4E, 0x45,
        0x43, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    ];

    fn image_with(records: &[(usize, [u8; 32])]) -> Vec<u8> {
        let mut image = vec![0xFFu8; IMAGE_LEN];
        for (slot, rec) in records {
            image[slot * ENTRY_LEN..(slot + 1) * ENTRY_LEN].copy_from_slice(rec);
        }
        image
    }

    /// The step-3 gate: decode the radio's own bytes and get the values the
    /// radio shows on its screen. Every field here was read off the radio.
    #[test]
    fn decodes_the_radios_own_records() {
        let image = image_with(&[
            (0, RADIO_CH1),
            (1, PROBE_TONEA),
            (2, PROBE_TONEB),
            (3, PROBE_TONEC),
        ]);
        let ch = decode_channels(&image);
        assert_eq!(ch.len(), 4);

        assert_eq!(ch[0].name, "PLUG73");
        assert_eq!(ch[0].rx_mhz, 145.100);
        assert_eq!(ch[0].zone, 1);

        assert_eq!(ch[1].name, "TONEA");
        assert_eq!(ch[1].rx_mhz, 146.520);
        assert_eq!(ch[1].rx_tone, "T 88.5");
        assert_eq!(ch[1].power, "High");
        assert!(!ch[1].narrow);

        assert_eq!(ch[2].rx_mhz, 146.940);
        assert_eq!(ch[2].tx_mhz, 146.340);
        assert_eq!(ch[2].rx_tone, "DTCS 023 N");
        assert_eq!(ch[2].power, "Low");
        assert!(ch[2].narrow);

        assert_eq!(ch[3].rx_tone, "T 141.3");
        assert_eq!(ch[3].tx_tone, "DTCS 754 I");
        assert_eq!(ch[3].power, "Middle");
    }

    /// The most valuable test in the process: re-encode the radio's own records
    /// from decoded values and get the radio's own bytes back.
    #[test]
    fn frequency_codec_round_trips_the_radios_bytes() {
        for rec in [RADIO_CH1, PROBE_TONEA, PROBE_TONEB, PROBE_TONEC] {
            let rx = lbcd_to_hz(&rec[0..4]);
            let tx = lbcd_to_hz(&rec[4..8]);
            assert_eq!(hz_to_lbcd(rx), rec[0..4], "rx re-encode");
            assert_eq!(hz_to_lbcd(tx), rec[4..8], "tx re-encode");
        }
    }

    /// The probe wrote both ends of the 210-entry table and the radio displayed
    /// `D023N` and `D754I`. Lock that, because an off-by-one here puts a
    /// different but entirely plausible tone on every DCS channel.
    #[test]
    fn dcs_table_ends_match_the_radio() {
        assert_eq!(dcs::dcs_to_byte(23, false), Some(1));
        assert_eq!(dcs::dcs_to_byte(754, true), Some(210));
        assert_eq!(dcs::byte_to_dcs(1), Some((23, false)));
        assert_eq!(dcs::byte_to_dcs(210), Some((754, true)));
        assert_eq!(dcs::byte_to_dcs(0), None);
        assert_eq!(dcs::DCS_TABLE.len(), 210);
    }

    /// CTCSS and DCS share two bytes and are told apart by the high byte being
    /// zero. Prove they cannot collide across the radio's whole CTCSS range:
    /// the lowest tone, 67.0 Hz, already has a non-zero high byte.
    #[test]
    fn ctcss_and_dcs_encodings_cannot_collide() {
        for tenths in 670..=2541 {
            let raw = encode_ctcss(f64::from(tenths) / 10.0);
            assert_ne!(raw[1], 0, "CTCSS {tenths} would decode as DCS");
        }
        assert_eq!(encode_ctcss(88.5), [0x75, 0x03]);
        assert_eq!(encode_ctcss(141.3), [0x85, 0x05]);
    }

    /// Blank and named channels use *different* pad bytes on this radio.
    /// A channel with no mode is wide FM, because that is what
    /// `export::channel_fit` decided when it let the channel through. The two
    /// used to disagree, and the encoder silently narrowed it.
    #[test]
    fn a_null_mode_encodes_as_wide_fm() {
        let wide = Channel { rx_freq: 146.52, ..Default::default() };
        assert_eq!(encode_channel(&wide, "NOMODE", 146_520_000, true)[15] & 0x40, 0x00);

        let fm = Channel { rx_freq: 146.52, mode: Some("FM".into()), ..Default::default() };
        assert_eq!(encode_channel(&fm, "FM", 146_520_000, true)[15] & 0x40, 0x00);

        let nfm = Channel { rx_freq: 146.52, mode: Some("NFM".into()), ..Default::default() };
        assert_eq!(encode_channel(&nfm, "NFM", 146_520_000, true)[15] & 0x40, 0x40);

        // Round-trips through the decoder, which knows only these two.
        for (mode, want) in [(None, "FM"), (Some("FM"), "FM"), (Some("NFM"), "NFM")] {
            let c = Channel {
                rx_freq: 146.52,
                mode: mode.map(str::to_string),
                ..Default::default()
            };
            let rec = encode_channel(&c, "X", 146_520_000, true);
            let mut image = vec![0xFFu8; IMAGE_LEN];
            image[..ENTRY_LEN].copy_from_slice(&rec);
            assert_eq!(decode_channels(&image)[0].narrow, want == "NFM", "{mode:?}");
        }
    }

    /// ⚠ MEASURED on the radio (s128), not inherited: a channel written with
    /// byte 15 bit 1 clear refuses the PTT while its neighbour with the bit set
    /// keys normally. Before that check this driver set the bit unconditionally,
    /// because clearing an unverified bit on this platform is how radios have
    /// been damaged.
    #[test]
    fn a_receive_only_channel_is_written_with_the_ptt_disabled() {
        let c = Channel { rx_freq: 146.52, mode: Some("FM".into()), ..Default::default() };
        assert_eq!(encode_channel(&c, "TX", 146_520_000, true)[15] & 0x02, 0x02);
        assert_eq!(encode_channel(&c, "RX", 146_520_000, false)[15] & 0x02, 0x00);
        // The bandwidth bit is independent of it.
        let n = Channel { mode: Some("NFM".into()), ..c.clone() };
        assert_eq!(encode_channel(&n, "RX", 146_520_000, false)[15], 0x40);
    }

    #[test]
    fn names_use_the_radios_two_sentinels() {
        assert_eq!(name_bytes(""), [0x00; NAME_LEN]);
        assert_eq!(&name_bytes("PLUG73")[..], &RADIO_CH1[20..32]);
        assert_eq!(decode_name(&RADIO_CH1[20..32]), "PLUG73");
        assert_eq!(decode_name(&[0x00; NAME_LEN]), "");
    }

    /// The guard that keeps a future edit from restoring the segment table that
    /// damaged a radio on this platform.
    #[test]
    fn write_segments_never_reach_a_firmware_managed_address() {
        assert_writable();
        assert!(forbidden(0x7800));
        assert!(forbidden(0x7FFF));
        assert!(forbidden(0x8080));
        assert!(forbidden(0x80FF));
        assert!(!forbidden(0x8000));
        assert!(!forbidden(0x807F));
    }

    /// A codeplug program leaves the radio holding the PROFILE's settings.
    ///
    /// It did not, and the reason is worth pinning: the whole-image upload has
    /// always addressed the function block, so declaring
    /// `carries_profile_settings = false` protected nothing — it just wrote the
    /// radio's own settings straight back over the profile's, every program.
    /// The three facts the fix stands on are all here.
    #[test]
    fn a_program_carries_the_profile_settings() {
        let caps = crate::radios::driver::DriverCapabilities::of(&DRIVER);
        assert!(
            caps.programs_settings,
            "the program writes the function block either way; saying otherwise \
             sends the radio's old settings back over the profile's"
        );
        assert!(
            caps.write_settings,
            "the narrow standalone write stays — a third of a second against the \
             four minutes a full program takes"
        );

        // The block the settings encoder writes into is one the upload sends.
        let seg = WRITE_SEGMENTS
            .iter()
            .find(|s| s.file_offset == FUNCTION_OFFSET)
            .expect("the function block is part of a full program's write");
        assert!(FUNCTION_LIVE_LEN <= seg.length);

        // And a channel patch cannot reach it: the records stop first.
        const { assert!(CHANNEL_COUNT * ENTRY_LEN <= FUNCTION_OFFSET) };

        // The encoder lands inside that segment, so what it writes goes out.
        const SQUELCH_ADDR: usize = 0x00;
        let mut image = vec![0xFFu8; IMAGE_LEN];
        let (written, notes) = settings::apply_profile_settings(
            &mut image,
            &serde_json::json!({ "squelch": 5 }),
        )
        .unwrap();
        assert_eq!((written, notes.len()), (1, 0));
        assert_eq!(image[FUNCTION_OFFSET + SQUELCH_ADDR], 5);
    }

    /// The read layout defines the image; the write layout must be a strict
    /// subset of it, never reaching a byte the read never filled.
    #[test]
    fn segment_tables_agree() {
        assert_eq!(
            READ_SEGMENTS.iter().map(|s| s.length).sum::<usize>(),
            IMAGE_LEN
        );
        for w in WRITE_SEGMENTS {
            let r = READ_SEGMENTS
                .iter()
                .find(|r| r.name == w.name)
                .expect("every write segment is also read");
            assert_eq!(r.file_offset, w.file_offset);
            assert!(w.length <= r.length, "{} writes more than it reads", w.name);
            assert!(w.file_offset + w.length <= IMAGE_LEN);
        }
        assert!(
            !WRITE_SEGMENTS.iter().any(|s| s.name == "aprs"),
            "the BT-9000's APRS block is read-only: 0x55 answers 0x06 and never commits"
        );
    }

    /// Two different keys decoded the same radio image byte-for-byte, which is
    /// what proved this rule. Lock its symmetry and its skip conditions.
    #[test]
    fn xor_is_symmetric_and_skips_what_the_radio_skips() {
        let (_, key) = encryption_frame();
        assert_eq!(&key, b"CO 7");
        let original: Vec<u8> = (0..=255u8).collect();
        let mut buf = original.clone();
        apply_xor(&mut buf, &key);
        apply_xor(&mut buf, &key);
        assert_eq!(buf, original, "XOR must be its own inverse");

        // 0x00 and 0xFF pass through untouched wherever they appear.
        let mut sentinels = vec![0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF];
        apply_xor(&mut sentinels, &key);
        assert_eq!(sentinels, vec![0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF]);
    }

    #[test]
    fn image_length_is_enforced() {
        assert!(validate_image(&vec![0u8; IMAGE_LEN]).is_ok());
        assert!(validate_image(&vec![0u8; IMAGE_LEN + 1]).is_err());
        assert!(validate_image(&vec![0u8; 0x10000]).is_err());
    }

    #[test]
    fn zones_are_positional_only() {
        let mut records = Vec::new();
        for slot in [0usize, 63, 64, 959] {
            records.push((slot, RADIO_CH1));
        }
        let image = image_with(&records);
        let ch = decode_channels(&image);
        assert_eq!(ch[0].zone, 1);
        assert_eq!(ch[1].zone, 1);
        assert_eq!(ch[2].zone, 2);
        assert_eq!(ch[3].zone, ZONE_COUNT);
    }
}
