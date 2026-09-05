//! The TM-D710's **second** transport: the memory image behind `0M PROGRAM`.
//!
//! The rest of this driver talks to the radio in ASCII, one command per memory.
//! That is not the whole radio. `MU` carries 42 menu parameters and reaches
//! none of the 600-series, which is 32 APRS and TNC menus — the largest group
//! on the radio and the feature it is named for. Those live in a binary image
//! that MCP-2A reads, and this module is how to get at it.
//!
//! ```text
//! 0M PROGRAM              -> 0M          the display shows PROG MCP
//! R <addr:2 BE> <len:1>   -> W <addr:2> <len:1> <data…>   (len 0 = 256)
//!                            the HOST then sends 06 and the radio answers 06
//! W <addr:2 BE> <len:1> <data…>
//!                         -> 06          the RADIO acknowledges; nothing goes back
//! E                       -> 06 0D 00    back to normal
//! ```
//!
//! ## Three things that each cost a session to find
//!
//! 1. **The address is big-endian.** Published notes for this mode say little.
//!    `0x0000` is the same two bytes either way, so the error survives being
//!    tested and the whole dump comes back drifting one byte per block.
//! 2. **A read is acknowledged by the host; a write is acknowledged by the
//!    radio.** Nothing published mentions the read acknowledgement, and without
//!    it only the first `R` of a session ever answers. Getting the asymmetry
//!    backwards leaves the stream one byte out of step from then on.
//! 3. **`0x7F00` is a hole, not the end.** The radio answers nothing there and
//!    answers again at `0x8000`. A reader that walks forward until a request
//!    fails reports 32 512 bytes as "the image" and loses the 7 KB above it —
//!    which is exactly where the APRS settings are. CHIRP's clone-mode driver
//!    skips the same block with the comment `# Skip block 7f !!??`.
//!
//! ## ⚠ Entering this mode is not free
//!
//! Nothing here writes unless asked to, but `0M PROGRAM` puts `PROG MCP` on the
//! radio's display and **leaving it there strands the operator** — the radio
//! stops answering `ID`, which looks exactly like a dead cable, and only a
//! power cycle gets it back. [`ProgramMode`] therefore sends `E` from `Drop`,
//! so an early return or a panic still leaves the radio usable.
//!
//! ## A narrow write commits
//!
//! CHIRP uploads all 156 blocks wrapped in an invalidate/revalidate ritual —
//! `FF` over the first byte of the headers at `0x0000` and `0x8000`, every
//! block, then the saved headers back — so that an interrupted upload leaves an
//! image explicitly marked bad rather than a plausible mixture. That is the
//! right shape for a full upload and **it is not required to change one field**:
//! measured on Tim's radio, 42 bytes written to one status text survived leaving
//! program mode and re-entering, and showed up on the radio's own menu.

use serialport::SerialPort;
use std::time::{Duration, Instant};

/// The image is addressed by a 16-bit word, so this is its whole extent. A
/// buffer of this size means **a file offset is a radio address**, which is the
/// only convention worth measuring in: CHIRP concatenates the blocks it read,
/// which silently shifts everything above the hole down by `0x100`.
pub(crate) const IMAGE_SPAN: usize = 0x1_0000;

/// The one block in `0x00`-`0x9B` the radio does not answer.
pub(crate) const HOLE: u16 = 0x7F00;

/// The live APRS/TNC settings. Five more copies follow at `+ n * 0x480`, one
/// per PM profile; those are the operator's saved configurations and this
/// driver has no business writing them.
pub(crate) const APRS_LIVE: u16 = 0x8100;

/// Bytes per APRS/TNC config block.
pub(crate) const APRS_BLOCK_LEN: usize = 0x480;

/// Long enough for a 256-byte block at 57 600 baud with room to spare, short
/// enough that the hole at `0x7F00` is diagnosed rather than waited on.
const BLOCK_TIMEOUT: Duration = Duration::from_millis(1200);

/// Every request MCP-2A makes, in its order: 256-byte blocks `0x00`-`0x9B`
/// except the hole, then two odd tails.
///
/// `len` of `0` means 256 — the radio's own convention, not ours.
pub(crate) fn read_plan() -> Vec<(u16, u8)> {
    let mut plan: Vec<(u16, u8)> = (0u16..0x9C)
        .map(|b| b << 8)
        .filter(|addr| *addr != HOLE)
        .map(|addr| (addr, 0u8))
        .collect();
    plan.push((0xFEF0, 0x10));
    plan.push((0xFF00, 0x90));
    plan
}

/// What came back, laid out at the addresses it came from.
pub(crate) struct Image {
    bytes: Vec<u8>,
    /// Address ranges actually answered, so a caller cannot mistake the `FF`
    /// filler for a region the radio really holds `FF` in.
    read: Vec<(u32, u32)>,
}

impl Image {
    /// The bytes at `addr`, or an error naming what was not read.
    ///
    /// ⚠ The distinction matters more here than on a clone radio: this image is
    /// mostly holes, and `FF` is also a perfectly ordinary stored value — an
    /// empty status text is 42 of them. Reading unread filler as data would put
    /// "the field is empty" and "the field was never fetched" into the same
    /// answer.
    pub(crate) fn slice(&self, addr: u16, len: usize) -> Result<&[u8], String> {
        let start = addr as u32;
        let end = start + len as u32;
        if end as usize > IMAGE_SPAN {
            return Err(format!("0x{addr:04X}+{len} runs past the end of the image"));
        }
        if !self.read.iter().any(|(a, b)| *a <= start && end <= *b) {
            return Err(format!(
                "0x{addr:04X}..0x{:04X} was never read from the radio",
                end.saturating_sub(1)
            ));
        }
        Ok(&self.bytes[start as usize..end as usize])
    }

    /// Total bytes the radio answered with.
    pub(crate) fn bytes_read(&self) -> usize {
        self.read.iter().map(|(a, b)| (b - a) as usize).sum()
    }

    /// The whole buffer, `FF` where nothing was read — for writing a dump file
    /// whose offsets are addresses.
    pub(crate) fn as_addressed_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// A program-mode session. Exits on drop.
pub(crate) struct ProgramMode<'a> {
    port: &'a mut dyn SerialPort,
    inside: bool,
}

impl<'a> ProgramMode<'a> {
    /// `0M PROGRAM`, tolerating the one `?` this radio can answer when the
    /// previous command left its parser mid-line.
    pub(crate) fn enter(port: &'a mut dyn SerialPort) -> Result<Self, String> {
        let reply = super::ask_settling(port, "0M PROGRAM")?;
        if !reply.starts_with("0M") {
            return Err(format!(
                "the radio refused program mode, answering {reply:?}. It has to be on and idle — \
                 not already in PROG MCP from an earlier run."
            ));
        }
        Ok(Self { port, inside: true })
    }

    /// One block. `len` of `0` asks for 256, which is the radio's convention.
    pub(crate) fn read(&mut self, addr: u16, len: u8) -> Result<Vec<u8>, String> {
        let req = [b'R', (addr >> 8) as u8, (addr & 0xFF) as u8, len];
        self.send(&req)?;

        let mut head = [0u8; 4];
        self.fill(&mut head).map_err(|e| format!("reading 0x{addr:04X}: {e}"))?;
        if head[0] != b'W' {
            return Err(format!(
                "reading 0x{addr:04X}: expected a W header, got {head:02X?}. \
                 A stream one byte out of step looks exactly like this."
            ));
        }
        let n = if head[3] == 0 { 256 } else { head[3] as usize };
        let mut data = vec![0u8; n];
        self.fill(&mut data).map_err(|e| format!("reading 0x{addr:04X}: {e}"))?;

        // ★ The host acknowledges a read. Skip it and the next request is never
        // answered — which reads like a refusal and is not.
        self.send(&[0x06])?;
        let mut status = [0u8; 1];
        self.fill(&mut status).map_err(|e| format!("acknowledging 0x{addr:04X}: {e}"))?;
        check_status(status[0])?;
        Ok(data)
    }

    /// One block, written. 1..=256 bytes at any address — this radio does not
    /// require block alignment and does not require the header dance.
    pub(crate) fn write(&mut self, addr: u16, data: &[u8]) -> Result<(), String> {
        if data.is_empty() || data.len() > 256 {
            return Err(format!("a block is 1..=256 bytes, not {}", data.len()));
        }
        let len = if data.len() == 256 { 0u8 } else { data.len() as u8 };
        let mut req = vec![b'W', (addr >> 8) as u8, (addr & 0xFF) as u8, len];
        req.extend_from_slice(data);
        self.send(&req)?;

        // ★ And here the RADIO acknowledges, with nothing to send back. The
        // asymmetry with `read` is the whole framing trap.
        let mut status = [0u8; 1];
        self.fill(&mut status).map_err(|e| format!("writing 0x{addr:04X}: {e}"))?;
        check_status(status[0]).map_err(|e| format!("writing 0x{addr:04X}: {e}"))
    }

    /// The whole image, every request in [`read_plan`].
    pub(crate) fn read_image(&mut self) -> Result<Image, String> {
        let mut bytes = vec![0xFFu8; IMAGE_SPAN];
        let mut read = Vec::new();
        for (addr, len) in read_plan() {
            let data = self.read(addr, len)?;
            let start = addr as u32;
            let end = start + data.len() as u32;
            bytes[start as usize..end as usize].copy_from_slice(&data);
            read.push((start, end));
        }
        Ok(Image { bytes, read })
    }

    /// `E`, checked. Prefer this to letting the session drop, which cannot
    /// report a failure.
    pub(crate) fn leave(mut self) -> Result<(), String> {
        self.exit()
    }

    fn exit(&mut self) -> Result<(), String> {
        if !self.inside {
            return Ok(());
        }
        self.inside = false;
        self.send(b"E")?;
        let mut ack = [0u8; 3];
        // The radio answers `06 0D 00`. A short read here is worth reporting
        // but not worth failing an otherwise good session over.
        match self.fill(&mut ack) {
            Ok(()) if ack[0] == 0x06 => Ok(()),
            Ok(()) => Err(format!("leaving program mode: the radio answered {ack:02X?}")),
            Err(e) => Err(format!("leaving program mode: {e}")),
        }
    }

    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.port.write_all(bytes).map_err(|e| e.to_string())?;
        self.port.flush().map_err(|e| e.to_string())
    }

    fn fill(&mut self, buf: &mut [u8]) -> Result<(), String> {
        let deadline = Instant::now() + BLOCK_TIMEOUT;
        let mut got = 0;
        while got < buf.len() {
            if Instant::now() >= deadline {
                return Err(format!("timed out after {got} of {} bytes", buf.len()));
            }
            match self.port.read(&mut buf[got..]) {
                Ok(0) => continue,
                Ok(n) => got += n,
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }
}

impl Drop for ProgramMode<'_> {
    fn drop(&mut self) {
        // ⚠ Not tidiness. A radio left in PROG MCP answers nothing at all, and
        // the operator's next move is to start unplugging the cable.
        let _ = self.exit();
    }
}

/// The one-byte status this mode answers with.
fn check_status(b: u8) -> Result<(), String> {
    match b {
        0x06 => Ok(()),
        // Published, and worth naming rather than reporting as a refusal: the
        // radio drops into this when the host leaves it idle in program mode
        // and the display changes to PROG ERR. It says nothing about the
        // command in hand, so looking for a validation rule here is a dead end.
        0x0F => Err("the radio is in the program-mode error state (PROG ERR); \
                     leave and re-enter program mode"
            .into()),
        other => Err(format!("the radio answered with status {other:02X}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::fake_port::{FakePort, FakeRadio};

    /// A TM-D710 that speaks both halves: ASCII until `0M PROGRAM`, binary
    /// after it.
    ///
    /// ⚠ What this cannot prove is that the wire format is right — it replays
    /// the format measured on Tim's radio. What it proves is the SEQUENCING:
    /// that the hole is skipped, that a read is acknowledged and a write is
    /// not, that `E` goes out even when the caller never asks, and that an
    /// unread region is refused rather than served as `FF`.
    struct FakeProgD710 {
        prog: bool,
        mem: Vec<u8>,
        /// Requests seen in program mode, for asserting the sequence.
        pub seen: Vec<String>,
        /// Answer nothing at the hole, exactly as the radio does.
        pub hole_is_silent: bool,
        /// Acknowledge a write and keep the old bytes — the failure that
        /// "0x06 came back" cannot distinguish from success.
        pub stubborn: bool,
    }

    impl FakeProgD710 {
        fn new() -> Self {
            let mut mem = vec![0xFFu8; IMAGE_SPAN];
            // Something recognisable in the APRS block and in block 0.
            mem[0..4].copy_from_slice(&[0x03, 0x4B, 0x01, 0xFF]);
            mem[APRS_LIVE as usize..APRS_LIVE as usize + 6].copy_from_slice(b"WW8L-1");
            Self { prog: false, mem, seen: Vec::new(), hole_is_silent: true, stubborn: false }
        }
    }

    impl FakeRadio for FakeProgD710 {
        fn step(&mut self, req: &[u8], out: &mut Vec<u8>) -> usize {
            if !self.prog {
                let Some(end) = req.iter().position(|&b| b == b'\r') else {
                    return 0;
                };
                let cmd = String::from_utf8_lossy(&req[..end]).into_owned();
                let reply = if cmd == "ID" {
                    "ID TM-D710".to_string()
                } else if cmd == "0M PROGRAM" {
                    self.prog = true;
                    "0M".to_string()
                } else {
                    "?".to_string()
                };
                out.extend_from_slice(reply.as_bytes());
                out.push(b'\r');
                return end + 1;
            }

            match req.first() {
                None => 0,
                Some(b'E') => {
                    self.prog = false;
                    self.seen.push("E".into());
                    out.extend_from_slice(&[0x06, 0x0D, 0x00]);
                    1
                }
                Some(0x06) => {
                    // The host acknowledging a read; the radio answers in kind.
                    out.push(0x06);
                    1
                }
                Some(b'R') => {
                    if req.len() < 4 {
                        return 0;
                    }
                    let addr = u16::from_be_bytes([req[1], req[2]]);
                    let n = if req[3] == 0 { 256 } else { req[3] as usize };
                    self.seen.push(format!("R {addr:04X} {n}"));
                    if self.hole_is_silent && addr == HOLE {
                        return 4; // consumed, and answered with nothing at all
                    }
                    out.extend_from_slice(&[b'W', req[1], req[2], req[3]]);
                    out.extend_from_slice(&self.mem[addr as usize..addr as usize + n]);
                    4
                }
                Some(b'W') => {
                    if req.len() < 4 {
                        return 0;
                    }
                    let n = if req[3] == 0 { 256 } else { req[3] as usize };
                    if req.len() < 4 + n {
                        return 0;
                    }
                    let addr = u16::from_be_bytes([req[1], req[2]]);
                    self.seen.push(format!("W {addr:04X} {n}"));
                    if !self.stubborn {
                        self.mem[addr as usize..addr as usize + n]
                            .copy_from_slice(&req[4..4 + n]);
                    }
                    out.push(0x06);
                    4 + n
                }
                Some(other) => panic!("the fake cannot classify {other:02X} in program mode"),
            }
        }
    }

    #[test]
    fn a_full_read_skips_the_hole_and_lands_every_block_at_its_own_address() {
        let mut port = FakePort::new(FakeProgD710::new());
        let image = {
            let mut prog = ProgramMode::enter(&mut port).expect("enter");
            let image = prog.read_image().expect("read the image");
            prog.leave().expect("leave");
            image
        };

        assert_eq!(image.bytes_read(), 39_840, "MCP's own ritual returns this many bytes");
        assert_eq!(image.slice(APRS_LIVE, 6).unwrap(), b"WW8L-1");
        assert_eq!(image.slice(0, 4).unwrap(), &[0x03, 0x4B, 0x01, 0xFF]);

        // The hole was never asked for, and its bytes are refused rather than
        // handed back as the FF they are filled with.
        assert!(!port.radio.seen.iter().any(|s| s.starts_with("R 7F00")));
        let err = image.slice(HOLE, 4).unwrap_err();
        assert!(err.contains("never read"), "{err}");
    }

    #[test]
    fn an_unread_region_is_refused_because_ff_is_also_a_real_value() {
        let mut port = FakePort::new(FakeProgD710::new());
        let mut prog = ProgramMode::enter(&mut port).expect("enter");
        let one = prog.read(APRS_LIVE, 16).expect("one block");
        assert_eq!(&one[..6], b"WW8L-1");
        // Reading one block does not make the rest of the image available.
        let image = Image { bytes: vec![0xFF; IMAGE_SPAN], read: vec![] };
        assert!(image.slice(APRS_LIVE, 1).is_err());
    }

    #[test]
    fn a_read_is_acknowledged_by_the_host_and_a_write_is_not() {
        let mut port = FakePort::new(FakeProgD710::new());
        {
            let mut prog = ProgramMode::enter(&mut port).expect("enter");
            prog.read(APRS_LIVE, 4).expect("read");
            prog.write(APRS_LIVE, b"K0AA").expect("write");
            prog.leave().expect("leave");
        }
        // If the driver had acknowledged the write too, the fake would have
        // answered that stray 0x06 and the next request would be one byte out.
        assert_eq!(
            port.radio.seen,
            vec!["R 8100 4".to_string(), "W 8100 4".to_string(), "E".to_string()]
        );
        assert_eq!(&port.radio.mem[APRS_LIVE as usize..APRS_LIVE as usize + 4], b"K0AA");
    }

    #[test]
    fn leaving_program_mode_happens_even_when_the_caller_never_asks() {
        let mut port = FakePort::new(FakeProgD710::new());
        {
            let mut prog = ProgramMode::enter(&mut port).expect("enter");
            let _ = prog.read(0, 4);
            // No `leave`. A caller that returns early, or panics, must not
            // strand the radio in PROG MCP — it stops answering ID there and
            // only a power cycle brings it back.
        }
        assert_eq!(port.radio.seen.last().map(String::as_str), Some("E"));
        assert!(!port.radio.prog, "the radio is still in program mode");
    }

    #[test]
    fn the_hole_is_reported_as_a_timeout_rather_than_hanging_the_read() {
        let mut port = FakePort::new(FakeProgD710::new());
        let mut prog = ProgramMode::enter(&mut port).expect("enter");
        let err = prog.read(HOLE, 0).unwrap_err();
        assert!(err.contains("timed out"), "{err}");
        assert!(err.contains("7F00"), "the address is what makes it diagnosable: {err}");
    }

    #[test]
    fn a_write_that_is_acknowledged_but_not_stored_is_only_visible_on_read_back() {
        let mut port = FakePort::new(FakeProgD710::new());
        port.radio.stubborn = true;
        let mut prog = ProgramMode::enter(&mut port).expect("enter");
        // ⚠ The write itself SUCCEEDS. 0x06 came back, which is all the
        // protocol offers. On the BT-9000 a segment behaved exactly like this
        // four times over, and only reading the bytes back showed it.
        prog.write(APRS_LIVE, b"K0AA").expect("the radio acknowledges");
        let back = prog.read(APRS_LIVE, 4).expect("read back");
        assert_eq!(back, b"WW8L".to_vec(), "the old value is still there");
    }

    #[test]
    fn the_program_mode_error_state_is_named_rather_than_reported_as_a_refusal() {
        let err = check_status(0x0F).unwrap_err();
        assert!(err.contains("PROG ERR"), "{err}");
        assert!(check_status(0x06).is_ok());
        assert!(check_status(0x15).unwrap_err().contains("15"));
    }

    #[test]
    fn the_read_plan_is_mcps_own_ritual() {
        let plan = read_plan();
        assert_eq!(plan.len(), 157, "155 blocks plus the two tails");
        assert!(!plan.iter().any(|(a, _)| *a == HOLE));
        assert_eq!(plan[0], (0x0000, 0));
        assert_eq!(plan[plan.len() - 2], (0xFEF0, 0x10));
        assert_eq!(plan[plan.len() - 1], (0xFF00, 0x90));
        let bytes: usize =
            plan.iter().map(|(_, l)| if *l == 0 { 256 } else { *l as usize }).sum();
        assert_eq!(bytes, 39_840);
    }
}
