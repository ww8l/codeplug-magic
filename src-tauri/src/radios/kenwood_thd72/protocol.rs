//! The TH-D72 clone protocol: ASCII commands, then 256-byte blocks.
//!
//! ⚠⚠ **NONE OF THIS HAS TOUCHED A RADIO.** Every byte below is CHIRP's
//! `thd72.py` transcribed and cross-read against LA3QMA's command repo. That is
//! exactly the standing of every inherited claim this project has later found
//! to be wrong — four of them so far, twice in a source's headline claim, and
//! the TH-D72 research has already contradicted three more (`shouldbe32`, the
//! balance enum, and which tone bytes the radio leaves in an untoned memory).
//! Phase 5 is what makes any of this true.
//!
//! ## The two layers
//!
//! Before programming mode the radio speaks **ASCII**: `CMD\r` in, `REPLY\r`
//! back. `ID` names the model, `TY` the variant (and whether TX has been
//! hardware-extended), `FV 0` the firmware. Those three are what Phase 1 asks.
//!
//! `0M PROGRAM` — literally zero, capital M, space, `PROGRAM` — switches the
//! radio into clone mode. From that point it speaks **binary blocks**: 5-byte
//! headers, 256 bytes of payload, one ACK each, and a bare `E` to leave.
//!
//! The baud rate changes underneath: the handshake is acknowledged at the
//! ASCII rate and the clone itself runs at [`BAUD_CLONE`]. Getting that wrong
//! looks exactly like a dead cable.
//!
//! ## Why an upload takes a block list
//!
//! An upload writes **only the blocks it is handed**, so a codeplug can go on
//! the radio without touching the APRS, GPS, TNC or calibration regions.
//! `container.rs` tracks which blocks actually changed and produces that list;
//! this module refuses to write the calibration blocks at all. That is the
//! whole safety argument for programming this radio, and it is the reason none
//! of these functions takes a "write everything" shortcut.

use std::time::{Duration, Instant};

use serialport::{ClearBuffer, SerialPort};

use crate::error::MapErrString;
use crate::radios::driver::RadioIdentity;

use super::layout::{BLOCK_COUNT, BLOCK_LEN, CALIBRATION_BASE, IMAGE_LEN};

/// The rate the radio answers `ID` on, and the rate `0M PROGRAM` is
/// acknowledged at.
pub(crate) const BAUD_INITIAL: u32 = 9600;

/// The rate the block transfer runs at. Switched to immediately after the
/// programming handshake — the radio is already there by the time it replies.
pub(crate) const BAUD_CLONE: u32 = 57600;

/// CHIRP's `_detect_baud` sweeps these before giving up, so a radio that is not
/// at 9600 is a supported case rather than a dead port. Kept in CHIRP's order.
const BAUD_SWEEP: [u32; 4] = [9600, 19200, 38400, 57600];

const TIMEOUT: Duration = Duration::from_secs(1);

/// CHIRP's `command()` deadline for a whole ASCII reply.
const CMD_TIMEOUT: Duration = Duration::from_millis(500);

const ACK: u8 = 0x06;

/// The programming-mode handshake. Uppercase matters — LA3QMA's `0M_PROGRAM.md`
/// says so explicitly, and it is the sort of detail that turns into an
/// afternoon.
const PROGRAM: &str = "0M PROGRAM";

/// What the radio answers `PROGRAM` with.
const PROGRAM_ACK: &str = "0M";

/// Leaves clone mode. One byte, no terminator, no reply.
const END: u8 = b'E';

/// First 256-byte block of the per-radio region at the top of the image.
///
/// `0xFE00`-`0xFFFF` is byte-identical across every image taken from one radio
/// — including across a factory reset — and different for every radio. Three
/// distinct blobs across eight images, matching exactly the three distinct
/// radios that produced them. That is calibration or serial data. CHIRP never
/// writes those two blocks and neither does this driver.
const FIRST_CALIBRATION_BLOCK: usize = CALIBRATION_BASE / BLOCK_LEN;

// ============================================================
// Port
// ============================================================

/// Open the radio's port at the ASCII rate.
///
/// **Hardware flow control on macOS only.** CHIRP carries
/// `HARDWARE_FLOW = sys.platform == "darwin"` for this radio specifically —
/// not for its Kenwoods generally — which reads as a workaround for the way the
/// D72's own USB stack behaves under the Darwin driver rather than anything
/// about the protocol. This project's only development machine is a Mac, so the
/// macOS branch is the one that will be exercised first and the other is
/// untested by anyone here.
///
/// ⚠ The D72's built-in mini-USB enumerates as `/dev/cu.usbmodem*` on macOS,
/// which the `serialport` crate's own enumeration misses; `list_serial_ports`
/// already supplements from `/dev/cu.*`, so the port will be offered. Nothing to
/// do here — but if the radio never appears in the picker, that is the code to
/// look at, not this.
pub(crate) fn open_port(port: &str) -> Result<Box<dyn SerialPort>, String> {
    let flow = if cfg!(target_os = "macos") {
        serialport::FlowControl::Hardware
    } else {
        serialport::FlowControl::None
    };
    serialport::new(port, BAUD_INITIAL)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(flow)
        .timeout(TIMEOUT)
        .open()
        .map_err(|e| format!("could not open {port}: {e}"))
}

/// Read a single byte, or `None` on timeout (no data).
fn read_byte(p: &mut dyn SerialPort) -> Result<Option<u8>, String> {
    let mut b = [0u8; 1];
    match p.read(&mut b) {
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

// ============================================================
// ASCII command layer
// ============================================================

/// Send `cmd` and read the reply up to its `\r`. An empty string means the
/// radio said nothing before the deadline — which is what a wrong baud rate,
/// a wrong port, or a radio that is off all look like.
/// `pub(crate)` so the Phase 1 hardware harness can ask the radio arbitrary
/// questions (`PV`, `MU`, `ME`, `MN`) through the SAME command layer the driver
/// uses. A harness with its own framing would prove the radio answers something,
/// not that this file reads it correctly.
pub(crate) fn command(p: &mut dyn SerialPort, cmd: &str) -> Result<String, String> {
    p.write_all(format!("{cmd}\r").as_bytes()).estr()?;
    p.flush().estr()?;

    let deadline = Instant::now() + CMD_TIMEOUT;
    let mut data = Vec::new();
    while Instant::now() < deadline {
        match read_byte(p)? {
            Some(b'\r') => break,
            Some(b) => data.push(b),
            None => {
                // Nothing waiting. On a real port `read` has already blocked for
                // the port timeout, so this is not a spin; on a fake it returns
                // instantly and the deadline is what stops us.
                if !data.is_empty() {
                    break;
                }
            }
        }
    }
    Ok(String::from_utf8_lossy(&data).trim().to_string())
}

/// Find the rate the radio is actually on, leaving the port set to it, and
/// return its `ID` reply.
///
/// CHIRP does this before every clone in either direction, which is why it is
/// here rather than left to the caller: a D72 whose port rate has been changed
/// is otherwise indistinguishable from a dead cable.
fn detect_baud(p: &mut dyn SerialPort) -> Result<String, String> {
    let mut saw_something = false;
    for rate in BAUD_SWEEP {
        p.set_baud_rate(rate).estr()?;
        let _ = p.clear(ClearBuffer::All);
        // CHIRP wakes the radio with a pair of bare carriage returns and throws
        // away whatever complaint comes back.
        p.write_all(b"\r\r").estr()?;
        let _ = read_drain(p);

        let reply = command(p, "ID")?;
        if reply.is_empty() {
            continue;
        }
        saw_something = true;
        if let Some(token) = reply.strip_prefix("ID ") {
            return Ok(token.trim().to_string());
        }
        // A radio that answers "?" is on the right rate but was confused by the
        // wake-up bytes. CHIRP retries the ID once at this rate, and says it
        // almost always works.
        if reply.contains('?') {
            let retry = command(p, "ID")?;
            if let Some(token) = retry.strip_prefix("ID ") {
                return Ok(token.trim().to_string());
            }
        }
    }
    if saw_something {
        Err("something answered on this port but not with a Kenwood ID reply — \
             is this a TH-D72?"
            .into())
    } else {
        Err("no response on this port (radio off, wrong cable/port, or the D72 is \
             not in normal operating mode?)"
            .into())
    }
}

/// Swallow whatever is currently waiting. Used after the wake-up bytes.
fn read_drain(p: &mut dyn SerialPort) -> Result<(), String> {
    while read_byte(p)?.is_some() {}
    Ok(())
}

/// Identify the radio over the ASCII layer.
///
/// ⚠ **This deliberately does not check the model token.** Neither CHIRP nor
/// LA3QMA records what a real TH-D72 returns for `ID` — CHIRP's clone driver
/// reads the token and never compares it to anything — so a hard-coded string
/// here would be a guess with the power to refuse Tim's own radio. This project
/// has already shipped one guard that refused legitimate input, and the fix
/// order matters: **Phase 1 writes the real token down, and only then does a
/// model check go in.**
///
/// ★ Phase 1 ran on 2026-08-26. A real TH-D72A (firmware 1.08, `TY A,M,B,1`)
/// answers exactly `ID TH-D72` — no variant suffix, so the A/E/K difference
/// lives in `TY` and not here. The check is now in, as a PREFIX match.
///
/// ⚠ That is **one radio**. Two samples agreeing is not a rule in this project
/// and one is less; the prefix is deliberately loose so a firmware that appends
/// something still passes, and the thing it exists to refuse is a different
/// model on the same cable — a TH-D74 speaks this command set too, and the next
/// thing this driver does after identifying is write. If a legitimate TH-D72 is
/// ever refused here, this guard is wrong, not the radio.
pub(crate) fn identify(p: &mut dyn SerialPort) -> Result<RadioIdentity, String> {
    let token = detect_baud(p)?;
    if !token.starts_with("TH-D72") {
        return Err(format!(
            "the radio on this port identified as {token:?}, not a TH-D72. Writing a TH-D72 \
             codeplug to it could not work and might not be harmless — check the cable."
        ));
    }
    Ok(RadioIdentity {
        matched: token.clone(),
        ident_hex: hex(token.as_bytes()),
        ident_ascii: Some(token),
    })
}

/// Ask the radio which variant it is. `A,M,B,1` is a stock TH-D72A; the same
/// command reports a hardware-extended TX, which is the one thing that decides
/// this model's `tx_bands` and cannot be looked up anywhere else.
///
/// Not called by the driver yet — Phase 1 runs it and writes the answer into
/// the seed. It lives here so that session does not have to invent it.
pub(crate) fn variant(p: &mut dyn SerialPort) -> Result<String, String> {
    let reply = command(p, "TY")?;
    reply
        .strip_prefix("TY ")
        .map(|s| s.trim().to_string())
        .ok_or_else(|| format!("unexpected reply to TY: {reply:?}"))
}

/// Firmware version. LA3QMA's tables are for V1.10 and note commands that
/// vanished at V1.08, so this is not a footnote.
pub(crate) fn firmware(p: &mut dyn SerialPort) -> Result<String, String> {
    let reply = command(p, "FV 0")?;
    reply
        .strip_prefix("FV ")
        .map(|s| s.trim().to_string())
        .ok_or_else(|| format!("unexpected reply to FV: {reply:?}"))
}

// ============================================================
// Clone mode
// ============================================================

/// Put the radio into programming mode and re-rate the port for the transfer.
///
/// Both directions do this, so both get the baud detection that precedes it —
/// exactly as CHIRP's `sync_in`/`sync_out` do. The cost is one extra `ID`
/// round-trip; the alternative is a download that fails on a radio whose port
/// rate was changed, with nothing in the error to say so.
fn enter_program(p: &mut dyn SerialPort) -> Result<(), String> {
    detect_baud(p)?;

    let reply = command(p, PROGRAM)?;
    if reply.is_empty() {
        return Err("no response to the programming command — the radio answered \
                    ID but will not enter clone mode."
            .into());
    }
    if reply != PROGRAM_ACK {
        return Err(format!(
            "radio refused clone mode: expected {PROGRAM_ACK:?}, got {reply:?}"
        ));
    }

    // The radio is already at the clone rate by the time that reply lands.
    p.set_baud_rate(BAUD_CLONE).estr()?;
    p.write_request_to_send(true).estr()?;

    // CHIRP reads and discards one byte here. Whatever it is, it is not part of
    // any frame — so this is best-effort: a radio that sends nothing must not
    // stall the clone waiting for it.
    let _ = read_byte(p)?;
    Ok(())
}

/// Leave clone mode. One byte, no reply expected.
fn end_session(p: &mut dyn SerialPort) -> Result<(), String> {
    p.write_all(&[END]).estr()?;
    p.flush().estr()
}

/// Request one 256-byte block.
///
/// Frame is `R`, `0x00`, the block number as a **little-endian u16**, `0x00`.
/// The reply echoes the same shape with `W` in front, and the echoed block
/// number is checked: a radio that answers with a different block than the one
/// asked for would otherwise assemble a plausible image out of the wrong bytes.
fn read_block(p: &mut dyn SerialPort, block: usize) -> Result<Vec<u8>, String> {
    let n = block as u16;
    p.write_all(&[b'R', 0x00, (n & 0xFF) as u8, (n >> 8) as u8, 0x00]).estr()?;

    let header = read_exact(p, 5)?;
    let cmd = header[0];
    let echoed = u16::from_le_bytes([header[2], header[3]]) as usize;
    if cmd != b'W' || echoed != block {
        return Err(format!(
            "bad header for block {block}: cmd={cmd:#04x} block={echoed} \
             (expected 'W' and block {block})"
        ));
    }

    let data = read_exact(p, BLOCK_LEN)?;

    p.write_all(&[ACK]).estr()?;
    if read_byte(p)? != Some(ACK) {
        return Err(format!("no post-block ack after block {block}"));
    }
    Ok(data)
}

/// Send one 256-byte block. `Ok(false)` is a NAK — the radio refused *this*
/// block, which is a different thing from the link failing.
fn write_block(p: &mut dyn SerialPort, block: usize, data: &[u8]) -> Result<bool, String> {
    debug_assert_eq!(data.len(), BLOCK_LEN);
    let n = block as u16;
    p.write_all(&[b'W', 0x00, (n & 0xFF) as u8, (n >> 8) as u8, 0x00]).estr()?;
    p.write_all(data).estr()?;
    p.flush().estr()?;

    Ok(read_byte(p)? == Some(ACK))
}

/// Read the whole 64 KiB image.
pub(crate) fn download(p: &mut dyn SerialPort) -> Result<Vec<u8>, String> {
    enter_program(p)?;

    let mut image = Vec::with_capacity(IMAGE_LEN);
    for block in 0..BLOCK_COUNT {
        match read_block(p, block) {
            Ok(data) => image.extend_from_slice(&data),
            Err(e) => {
                // Best-effort: try to leave the radio in a sane state before
                // reporting. A download cannot damage anything, so the read
                // error is what matters and this must not mask it.
                let _ = end_session(p);
                return Err(format!("{e} (read {} of {BLOCK_COUNT} blocks)", image.len() / BLOCK_LEN));
            }
        }
    }

    end_session(p)?;
    Ok(image)
}

/// Write **only** `blocks` back to the radio.
///
/// Every block is validated before the first byte goes out: a bad list is a
/// programming error, and finding out halfway through would leave the radio
/// holding a mix of two codeplugs for no reason at all.
pub(crate) fn upload(
    p: &mut dyn SerialPort,
    image: &[u8],
    blocks: &[usize],
) -> Result<(), String> {
    if image.len() != IMAGE_LEN {
        return Err(format!(
            "image is {} bytes — a TH-D72 image is {IMAGE_LEN}.",
            image.len()
        ));
    }
    for &block in blocks {
        if block >= FIRST_CALIBRATION_BLOCK {
            return Err(format!(
                "refusing to write block {block} ({:#06X}-{:#06X}): blocks \
                 {FIRST_CALIBRATION_BLOCK} and up hold per-radio data — identical across every \
                 image from one radio including a factory reset, and different for every \
                 radio. CHIRP never writes them and neither does this driver.",
                block * BLOCK_LEN,
                block * BLOCK_LEN + BLOCK_LEN - 1
            ));
        }
    }
    if blocks.is_empty() {
        // Nothing changed. Entering clone mode to write zero blocks would put
        // the radio through a mode change for no reason.
        return Ok(());
    }

    enter_program(p)?;

    for (already_written, &block) in blocks.iter().enumerate() {
        let off = block * BLOCK_LEN;
        let acked = write_block(p, block, &image[off..off + BLOCK_LEN])?;
        if !acked {
            let _ = end_session(p);
            return Err(format!(
                "the radio refused block {block}. {already_written} of {} blocks had already \
                 been written, so it is now holding a MIX of its old codeplug and the new one — \
                 re-run the program, or restore the pre-write backup.",
                blocks.len()
            ));
        }
    }

    end_session(p)?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::fake_port::{FakePort, FakeRadio};

    /// A TH-D72's side of the conversation.
    ///
    /// ⚠ Same limit as every fake in this repo, and worth restating because
    /// this file is the one place it bites hardest: **this cannot prove the
    /// wire format.** It replays the frames `protocol.rs` believes in, so if
    /// CHIRP's transcription is wrong, the driver and this fake are wrong
    /// together and every test below still passes. What it does prove is the
    /// SEQUENCING — that the handshake precedes the blocks, that exactly the
    /// requested blocks are written and no others, that `E` is sent, and what
    /// happens when the radio refuses block 37. A real radio cannot be asked to
    /// refuse block 37 on demand, so that half was never checked on hardware
    /// either.
    struct FakeThd72 {
        /// The radio's memory. 64 KiB, `0xFF` where nothing was seeded.
        image: Vec<u8>,
        /// True once `0M PROGRAM` has been accepted.
        in_program: bool,
        /// What to answer `ID` with, verbatim.
        id_reply: String,
        /// What to answer `0M PROGRAM` with, verbatim.
        program_reply: String,
        /// Emit a stray byte after the programming handshake, as CHIRP's
        /// discard-one-byte suggests a real radio does.
        stray_byte_after_program: bool,
        /// Echo the wrong block number in this block's read header.
        echo_wrong_block_at: Option<usize>,
        /// NAK the write of this block.
        nak_write_at: Option<usize>,
        /// Blocks read, in order.
        reads: Vec<usize>,
        /// Blocks written, in order, with their payload.
        writes: Vec<(usize, Vec<u8>)>,
        /// True once `E` has been seen.
        ended: bool,
    }

    impl FakeThd72 {
        fn new() -> Self {
            Self {
                image: vec![0xFF; IMAGE_LEN],
                in_program: false,
                id_reply: "ID TH-D72".into(),
                program_reply: PROGRAM_ACK.into(),
                stray_byte_after_program: false,
                echo_wrong_block_at: None,
                nak_write_at: None,
                reads: Vec::new(),
                writes: Vec::new(),
                ended: false,
            }
        }

        fn answering_id(mut self, reply: &str) -> Self {
            self.id_reply = reply.into();
            self
        }

        fn refusing_program(mut self, reply: &str) -> Self {
            self.program_reply = reply.into();
            self
        }

        /// Seed a block so a download has something distinguishable in it.
        fn seed_block(&mut self, block: usize, fill: u8) {
            let off = block * BLOCK_LEN;
            self.image[off..off + BLOCK_LEN].fill(fill);
        }

        /// One complete ASCII command, or `None` if `\r` has not arrived yet.
        fn ascii_command(req: &[u8]) -> Option<(String, usize)> {
            let at = req.iter().position(|&b| b == b'\r')?;
            Some((String::from_utf8_lossy(&req[..at]).trim().to_string(), at + 1))
        }
    }

    impl FakeRadio for FakeThd72 {
        fn step(&mut self, req: &[u8], out: &mut Vec<u8>) -> usize {
            let Some(&first) = req.first() else {
                return 0;
            };

            if !self.in_program {
                // A bare carriage return is the wake-up; the radio shrugs.
                if first == b'\r' {
                    return 1;
                }
                let Some((cmd, used)) = Self::ascii_command(req) else {
                    return 0;
                };
                match cmd.as_str() {
                    "ID" => out.extend_from_slice(format!("{}\r", self.id_reply).as_bytes()),
                    "TY" => out.extend_from_slice(b"TY A,M,B,1\r"),
                    "FV 0" => out.extend_from_slice(b"FV 1.10\r"),
                    PROGRAM => {
                        out.extend_from_slice(format!("{}\r", self.program_reply).as_bytes());
                        if self.program_reply == PROGRAM_ACK {
                            self.in_program = true;
                            if self.stray_byte_after_program {
                                out.push(0x00);
                            }
                        }
                    }
                    "" => {}
                    other => panic!("fake TH-D72 got an unknown ASCII command {other:?}"),
                }
                return used;
            }

            match first {
                b'R' => {
                    if req.len() < 5 {
                        return 0;
                    }
                    let block = u16::from_le_bytes([req[2], req[3]]) as usize;
                    assert_eq!(req[1], 0x00, "read frame byte 1 must be zero");
                    assert_eq!(req[4], 0x00, "read frame byte 4 must be zero");
                    self.reads.push(block);

                    let echoed = if self.echo_wrong_block_at == Some(block) {
                        block.wrapping_add(1)
                    } else {
                        block
                    } as u16;
                    out.push(b'W');
                    out.push(0x00);
                    out.extend_from_slice(&echoed.to_le_bytes());
                    out.push(0x00);
                    let off = block * BLOCK_LEN;
                    out.extend_from_slice(&self.image[off..off + BLOCK_LEN]);
                    5
                }
                b'W' => {
                    if req.len() < 5 + BLOCK_LEN {
                        return 0;
                    }
                    let block = u16::from_le_bytes([req[2], req[3]]) as usize;
                    let data = req[5..5 + BLOCK_LEN].to_vec();
                    if self.nak_write_at == Some(block) {
                        out.push(0x15); // anything that is not an ACK
                    } else {
                        let off = block * BLOCK_LEN;
                        self.image[off..off + BLOCK_LEN].copy_from_slice(&data);
                        self.writes.push((block, data));
                        out.push(ACK);
                    }
                    5 + BLOCK_LEN
                }
                // The host's post-read ack; the radio echoes it.
                ACK => {
                    out.push(ACK);
                    1
                }
                END => {
                    self.ended = true;
                    self.in_program = false;
                    1
                }
                other => panic!("fake TH-D72 got an unclassifiable byte {other:#04x} in clone mode"),
            }
        }
    }

    #[test]
    fn identify_reads_the_model_token_out_of_the_id_reply() {
        let mut p = FakePort::new(FakeThd72::new());
        let ident = identify(&mut p).expect("identify");
        assert_eq!(ident.matched, "TH-D72");
        assert_eq!(ident.ident_ascii.as_deref(), Some("TH-D72"));
    }

    /// ★ Inverted by Phase 1 on 2026-08-26. This test used to assert that a
    /// TH-D74 reply was *accepted*, because nothing in the research recorded
    /// what a real D72 answers and a guessed token could have refused the actual
    /// radio. The radio has now been asked: it says `ID TH-D72`. The guard is in
    /// and a sibling model is refused — which matters because the next thing
    /// this driver does after identifying is write.
    #[test]
    fn a_different_kenwood_on_the_cable_is_refused() {
        let mut p = FakePort::new(FakeThd72::new().answering_id("ID TH-D74"));
        let err = match identify(&mut p) {
            Ok(_) => panic!("a TH-D74 must not pass as a TH-D72"),
            Err(e) => e,
        };
        assert!(err.contains("TH-D74"), "the error must name what answered: {err}");
    }

    /// The prefix is loose on purpose: a firmware that appends a variant letter
    /// must still pass. Only the model is checked; A/E/K live in `TY`.
    #[test]
    fn a_variant_suffix_still_identifies_as_a_thd72() {
        let mut p = FakePort::new(FakeThd72::new().answering_id("ID TH-D72A"));
        assert_eq!(identify(&mut p).expect("identify").matched, "TH-D72A");
    }

    #[test]
    fn a_radio_that_does_not_answer_with_an_id_reply_is_refused() {
        let mut p = FakePort::new(FakeThd72::new().answering_id("NOPE"));
        // `RadioIdentity` has no `Debug`, so the error comes out by hand rather
        // than through `expect_err`.
        let err = identify(&mut p).err().expect("should refuse");
        assert!(err.contains("TH-D72"), "{err}");
    }

    #[test]
    fn a_silent_port_is_refused_rather_than_hanging() {
        let mut p = FakePort::new(FakeThd72::new().answering_id(""));
        let err = identify(&mut p).err().expect("should refuse");
        assert!(err.contains("no response"), "{err}");
    }

    #[test]
    fn a_radio_that_refuses_clone_mode_is_an_error_not_a_hang() {
        let mut p = FakePort::new(FakeThd72::new().refusing_program("?"));
        let err = download(&mut p).expect_err("should refuse");
        assert!(err.contains("refused clone mode"), "{err}");
    }

    #[test]
    fn a_full_download_reads_every_block_in_order_and_ends_the_session() {
        let mut radio = FakeThd72::new();
        radio.seed_block(0, 0xAA);
        radio.seed_block(BLOCK_COUNT - 1, 0x55);
        let mut p = FakePort::new(radio);

        let image = download(&mut p).expect("download");

        assert_eq!(image.len(), IMAGE_LEN);
        assert_eq!(image[0], 0xAA);
        assert_eq!(image[IMAGE_LEN - 1], 0x55);
        assert_eq!(p.radio.reads, (0..BLOCK_COUNT).collect::<Vec<_>>());
        assert!(p.radio.ended, "the session must be ended with E");
    }

    /// The stray byte CHIRP discards after the handshake must be tolerated, and
    /// so must its absence — the discard is best-effort for exactly this reason.
    #[test]
    fn the_post_handshake_stray_byte_is_tolerated_either_way() {
        for stray in [false, true] {
            let mut radio = FakeThd72::new();
            radio.stray_byte_after_program = stray;
            radio.seed_block(0, 0x42);
            let mut p = FakePort::new(radio);
            let image = download(&mut p).unwrap_or_else(|e| panic!("stray={stray}: {e}"));
            assert_eq!(image[0], 0x42, "stray={stray}");
        }
    }

    #[test]
    fn a_block_that_echoes_the_wrong_number_is_caught() {
        let mut radio = FakeThd72::new();
        radio.echo_wrong_block_at = Some(7);
        let mut p = FakePort::new(radio);

        let err = download(&mut p).expect_err("should catch the mismatch");
        assert!(err.contains("block 7"), "{err}");
        assert!(err.contains("read 7 of"), "{err}");
    }

    #[test]
    fn an_upload_writes_exactly_the_blocks_it_was_given() {
        let mut p = FakePort::new(FakeThd72::new());
        let mut image = vec![0x00; IMAGE_LEN];
        image[3 * BLOCK_LEN] = 0xC5;
        image[200 * BLOCK_LEN] = 0x9E;

        upload(&mut p, &image, &[3, 200]).expect("upload");

        let written: Vec<usize> = p.radio.writes.iter().map(|(b, _)| *b).collect();
        assert_eq!(written, vec![3, 200], "no block outside the list may be written");
        assert_eq!(p.radio.image[3 * BLOCK_LEN], 0xC5);
        assert_eq!(p.radio.image[200 * BLOCK_LEN], 0x9E);
        // Everything else is still the radio's own 0xFF fill.
        assert_eq!(p.radio.image[4 * BLOCK_LEN], 0xFF);
        assert!(p.radio.ended);
    }

    #[test]
    fn a_nak_names_the_block_and_says_how_much_was_already_written() {
        let mut radio = FakeThd72::new();
        radio.nak_write_at = Some(5);
        let mut p = FakePort::new(radio);

        let err = upload(&mut p, &vec![0u8; IMAGE_LEN], &[1, 2, 5, 9]).expect_err("should fail");
        assert!(err.contains("block 5"), "{err}");
        assert!(err.contains("2 of 4"), "{err}");
        assert!(err.contains("MIX"), "the operator must be told the radio is half-written: {err}");
        // The two blocks before the failure really did land.
        assert_eq!(
            p.radio.writes.iter().map(|(b, _)| *b).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn a_calibration_block_is_refused_before_anything_is_written() {
        for block in [FIRST_CALIBRATION_BLOCK, BLOCK_COUNT - 1] {
            let mut p = FakePort::new(FakeThd72::new());
            let err = upload(&mut p, &vec![0u8; IMAGE_LEN], &[1, block])
                .expect_err("calibration blocks must be refused");
            assert!(err.contains("per-radio data"), "{err}");
            assert!(
                p.radio.writes.is_empty(),
                "block 1 must not be written before the bad list is rejected"
            );
        }
    }

    #[test]
    fn an_upload_of_nothing_does_not_touch_the_radio() {
        let mut p = FakePort::new(FakeThd72::new());
        upload(&mut p, &vec![0u8; IMAGE_LEN], &[]).expect("no-op upload");
        assert!(p.radio.writes.is_empty());
        assert!(!p.radio.in_program, "the radio should never have entered clone mode");
    }

    #[test]
    fn an_image_of_the_wrong_length_is_refused() {
        let mut p = FakePort::new(FakeThd72::new());
        let err = upload(&mut p, &[0u8; 16], &[1]).expect_err("should refuse");
        assert!(err.contains("16 bytes"), "{err}");
    }

    #[test]
    fn the_variant_and_firmware_commands_parse_their_replies() {
        let mut p = FakePort::new(FakeThd72::new());
        // Both run on the ASCII layer, after the baud sweep has settled it.
        identify(&mut p).expect("identify");
        assert_eq!(variant(&mut p).expect("TY"), "A,M,B,1");
        assert_eq!(firmware(&mut p).expect("FV"), "1.10");
    }
}
