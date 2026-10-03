//! Icom clone protocol, as the ID-5100 speaks it over its data cable.
//!
//! Transcribed from CHIRP's `icf.py` (the ID-5100 is `_raw_frames`,
//! `_highbit_flip`, `_can_hispeed`), then run against the real radio in
//! `scratchpad/id5100/readradio.py` before this file existed: that reader's
//! image matched CHIRP's own download of the same radio in every byte but the
//! read counter. What that proves and what it does not:
//!
//! - **Download** — the frame format, escaping, checksum, hispeed switch, high-
//!   bit flip and end frame are measured (six reads, zero checksum failures).
//! - **Upload** — NOT run against a radio. It is CHIRP's sequence with the same
//!   framing the download proves, and the fake radio below checks its
//!   sequencing; the hardware ladder is what proves it.
//!
//! ## Frames
//!
//! `FE FE <src> <dst> <cmd> <payload> FD`, PC = `EE`, radio = `EF`. Payload
//! bytes above `F9` are escaped as `FF, b & 0x0F` so `FD`/`FE` never appear
//! inside one. Clone data frames carry `[addr u32 BE][len][data…][checksum]`,
//! the checksum being the two's complement of the byte sum of everything before
//! it. On the wire every image byte has its high bit flipped.
//!
//! The RT Systems cable Tim uses does **not** echo (measured: an ID query came
//! back as the radio's reply alone). Echoed frames from a cable that does are
//! skipped by source address anyway, as CHIRP does.

use std::time::Duration;

use serialport::{ClearBuffer, SerialPort};

use super::layout::{ENDFRAME, IMAGE_LEN, MAP_REV, MODEL};
use crate::radios::driver::RadioIdentity;

pub(crate) const BAUD_INITIAL: u32 = 9600;
pub(crate) const BAUD_CLONE: u32 = 38400;

const PC: u8 = 0xEE;
const RADIO: u8 = 0xEF;
const CMD_ID: u8 = 0xE0;
const CMD_MODEL: u8 = 0xE1;
const CMD_CLONE_OUT: u8 = 0xE2;
const CMD_CLONE_IN: u8 = 0xE3;
const CMD_DATA: u8 = 0xE4;
const CMD_END: u8 = 0xE5;
const CMD_OK: u8 = 0xE6;
const CMD_HISPEED: u8 = 0xE8;

/// Bytes per upload frame — CHIRP's `_ranges = [(0, memsize, 64)]`. The radio
/// sends 32 per frame on download; it is the writer's choice on upload.
const UPLOAD_CHUNK: usize = 64;

const TIMEOUT: Duration = Duration::from_millis(250);
/// Quiet reads before a stalled stream is given up on: 40 x 250 ms = CHIRP's
/// ten seconds.
const STALL_READS: usize = 40;

pub(crate) fn open_port(port: &str) -> Result<Box<dyn SerialPort>, String> {
    serialport::new(port, BAUD_INITIAL)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        .timeout(TIMEOUT)
        .open()
        .map_err(|e| format!("could not open {port}: {e}"))
}

fn frame(cmd: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = vec![0xFE, 0xFE, PC, RADIO, cmd];
    f.extend_from_slice(payload);
    f.push(0xFD);
    f
}

fn escape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 8);
    for &b in data {
        if b > 0xF9 {
            out.extend_from_slice(&[0xFF, b & 0x0F]);
        } else {
            out.push(b);
        }
    }
    out
}

fn unescape(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(data.len());
    let mut it = data.iter();
    while let Some(&b) = it.next() {
        if b == 0xFF {
            let &lo = it.next().ok_or("escape byte at the end of a frame")?;
            out.push(0xF0 | lo);
        } else {
            out.push(b);
        }
    }
    Ok(out)
}

fn checksum(data: &[u8]) -> u8 {
    let sum: u32 = data.iter().map(|&b| u32::from(b)).sum();
    ((sum ^ 0xFFFF) + 1) as u8
}

fn write(p: &mut dyn SerialPort, bytes: &[u8]) -> Result<(), String> {
    p.write_all(bytes).map_err(|e| format!("write to radio failed: {e}"))?;
    p.flush().map_err(|e| format!("write to radio failed: {e}"))
}

/// Radio-to-PC frames, reassembled from whatever the port delivers.
struct Frames {
    buf: Vec<u8>,
}

impl Frames {
    fn new() -> Self {
        Frames { buf: Vec::new() }
    }

    /// Read what is available. `false` means the port was quiet for one timeout.
    fn fill(&mut self, p: &mut dyn SerialPort) -> Result<bool, String> {
        let mut chunk = [0u8; 4096];
        match p.read(&mut chunk) {
            Ok(0) => Ok(false),
            Ok(n) => {
                self.buf.extend_from_slice(&chunk[..n]);
                Ok(true)
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(false),
            Err(e) => Err(format!("read from radio failed: {e}")),
        }
    }

    /// Next complete frame from the radio as `(cmd, raw payload)`. Frames the PC
    /// sent (an echoing cable) are dropped here.
    fn next(&mut self) -> Option<(u8, Vec<u8>)> {
        loop {
            let start = self.buf.windows(2).position(|w| w == [0xFE, 0xFE])?;
            self.buf.drain(..start);
            // Hispeed frames arrive behind a run of FE padding.
            while self.buf.len() >= 3 && self.buf[..3] == [0xFE, 0xFE, 0xFE] {
                self.buf.remove(0);
            }
            let end = self.buf.iter().position(|&b| b == 0xFD)?;
            let f: Vec<u8> = self.buf.drain(..=end).collect();
            if f.len() < 6 || f[2] == PC {
                continue;
            }
            return Some((f[4], f[5..f.len() - 1].to_vec()));
        }
    }
}

/// Ask the radio who it is, and refuse anything but an ID-5100 on the layout
/// this driver knows.
///
/// The model-data reply is 49 bytes; byte 5 is the memory-layout revision
/// (CHIRP `decode_model`). Measured `3` on firmware 1.21. A radio on an older
/// layout stores its memories at different offsets, and the next thing this
/// driver does after identifying may be to write — so it stops here.
pub(crate) fn identify(p: &mut dyn SerialPort) -> Result<RadioIdentity, String> {
    let _ = p.clear(ClearBuffer::Input);
    write(p, &frame(CMD_ID, &escape(&[0, 0, 0, 0])))?;
    let mut frames = Frames::new();
    let mut quiet = 0;
    let md = loop {
        if let Some((cmd, payload)) = frames.next() {
            if cmd == CMD_MODEL {
                break unescape(&payload)?;
            }
            continue;
        }
        if !frames.fill(p)? {
            quiet += 1;
            if quiet > 12 {
                return Err("the radio did not answer. Check the cable is in the data jack \
                            and the radio is on."
                    .into());
            }
        }
    };
    let hex = md.iter().map(|b| format!("{b:02x}")).collect::<String>();
    if md.len() < 6 || md[..4] != MODEL {
        return Err(format!(
            "the radio on this port is not an ID-5100 (it identified as {}). Writing an \
             ID-5100 codeplug to it could not work and might not be harmless.",
            &hex[..hex.len().min(8)]
        ));
    }
    if md[5] != MAP_REV {
        return Err(format!(
            "this ID-5100 reports memory layout revision {}, and CodePlug Magic knows only \
             revision {MAP_REV} — the layout current firmware uses. Older firmware stores \
             memories at different addresses, so nothing will be written. Updating the \
             radio's firmware from Icom's site moves it to revision {MAP_REV}.",
            md[5]
        ));
    }
    Ok(RadioIdentity {
        matched: "ID-5100".into(),
        ident_hex: hex,
        ident_ascii: None,
    })
}

/// Switch the session to 38400 baud and send `cmd` (clone out or in).
///
/// The switch frame's payload ends in a literal `FD` before the frame's own —
/// CHIRP's `start_hispeed_clone`, byte for byte — and the radio sends nothing
/// back (measured: zero reply bytes).
fn start_hispeed(p: &mut dyn SerialPort, cmd: u8) -> Result<(), String> {
    let mut payload = MODEL.to_vec();
    payload.extend_from_slice(&[0x00, 0x00, 0x02, 0x01, 0xFD]);
    let mut bytes = vec![0xFE; 20];
    bytes.extend(frame(CMD_HISPEED, &payload));
    write(p, &bytes)?;
    // CHIRP reads (and ignores) up to 128 bytes here, costing one timeout.
    let mut sink = Frames::new();
    sink.fill(p)?;
    p.set_baud_rate(BAUD_CLONE)
        .map_err(|e| format!("could not switch to {BAUD_CLONE} baud: {e}"))?;
    let mut bytes = vec![0xFE; 14];
    bytes.extend(frame(cmd, &[MODEL[0], MODEL[1], MODEL[2], 0x00]));
    write(p, &bytes)
}

/// Read the whole image. Call [`identify`] on the same port first.
///
/// Returns the image with the high bits already restored — the same bytes
/// CHIRP stores in an `.img`. Refuses a read with a bad frame checksum, a gap,
/// or an end frame other than the one this layout ends with.
pub(crate) fn download(p: &mut dyn SerialPort) -> Result<Vec<u8>, String> {
    start_hispeed(p, CMD_CLONE_OUT)?;
    let mut image = vec![0u8; IMAGE_LEN];
    let mut covered = vec![false; IMAGE_LEN];
    let mut frames = Frames::new();
    let mut quiet = 0;
    let end = loop {
        match frames.next() {
            Some((CMD_DATA, payload)) => {
                quiet = 0;
                let d = unescape(&payload)?;
                if d.len() < 6 {
                    return Err("a clone frame from the radio was too short".into());
                }
                let addr = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) as usize;
                let n = usize::from(d[4]);
                if d.len() != 5 + n + 1 {
                    return Err(format!("clone frame at {addr:#07x} has the wrong length"));
                }
                if checksum(&d[..5 + n]) != d[5 + n] {
                    return Err(format!("checksum error in the clone frame at {addr:#07x}"));
                }
                if addr + n > IMAGE_LEN {
                    return Err(format!(
                        "the radio sent data at {addr:#07x}, past the {IMAGE_LEN:#x}-byte image \
                         this driver knows — a different memory layout"
                    ));
                }
                image[addr..addr + n].copy_from_slice(&d[5..5 + n]);
                covered[addr..addr + n].iter_mut().for_each(|c| *c = true);
            }
            Some((CMD_END, payload)) => break unescape(&payload)?,
            Some(_) => {}
            None => {
                if !frames.fill(p)? {
                    quiet += 1;
                    if quiet > STALL_READS {
                        return Err("the radio stopped sending before the clone finished".into());
                    }
                }
            }
        }
    };
    if end != ENDFRAME {
        return Err(format!(
            "the clone ended with {:?}, not {:?} — a different memory layout",
            String::from_utf8_lossy(&end),
            String::from_utf8_lossy(ENDFRAME)
        ));
    }
    if let Some(gap) = covered.iter().position(|c| !c) {
        return Err(format!("the radio's clone skipped the bytes at {gap:#07x}"));
    }
    image.iter_mut().for_each(|b| *b ^= 0x80);
    Ok(image)
}

/// Write a whole image. Call [`identify`] on the same port first.
///
/// ⚠ Not yet run against a radio — see the module header.
pub(crate) fn upload(p: &mut dyn SerialPort, image: &[u8]) -> Result<(), String> {
    if image.len() != IMAGE_LEN {
        return Err(format!(
            "refusing to write a {}-byte image; an ID-5100 image is {IMAGE_LEN} bytes",
            image.len()
        ));
    }
    start_hispeed(p, CMD_CLONE_IN)?;
    let mut frames = Frames::new();
    for addr in (0..IMAGE_LEN).step_by(UPLOAD_CHUNK) {
        let n = UPLOAD_CHUNK.min(IMAGE_LEN - addr);
        let mut chunk = (addr as u32).to_be_bytes().to_vec();
        chunk.push(n as u8);
        chunk.extend(image[addr..addr + n].iter().map(|b| b ^ 0x80));
        chunk.push(checksum(&chunk));
        write(p, &frame(CMD_DATA, &escape(&chunk)))?;
        // Nothing is expected back mid-stream, but a radio that has something
        // to say should not have it pile up in the driver's buffer.
        if p.bytes_to_read().unwrap_or(0) > 0 {
            frames.fill(p)?;
        }
    }
    write(p, &frame(CMD_END, &escape(ENDFRAME)))?;

    let mut quiet = 0;
    loop {
        match frames.next() {
            Some((CMD_OK, payload)) => {
                let r = unescape(&payload)?;
                return if r.first() == Some(&0) {
                    Ok(())
                } else {
                    Err(format!(
                        "the radio refused the clone (result {:02x?}). It may be holding a \
                         partial write — restore the backup before using it.",
                        r
                    ))
                };
            }
            Some(_) => {}
            None => {
                if !frames.fill(p)? {
                    quiet += 1;
                    if quiet > 20 {
                        return Err("the radio took the whole image but never confirmed it. \
                                    Power-cycle it and read it back before trusting it."
                            .into());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::fake_port::{FakePort, FakeRadio};

    /// An ID-5100 at the far end of the cable: answers the ID query, streams its
    /// image on clone-out the way the real radio does (32-byte frames, high bits
    /// flipped), takes frames on clone-in, and confirms on end.
    struct FakeIcom {
        image: Vec<u8>,
        rev: u8,
        written: Vec<u8>,
        received: Vec<bool>,
        clone_in: bool,
        ended: bool,
        /// Corrupt the checksum of the download frame at this address.
        bad_checksum_at: Option<usize>,
    }

    impl FakeIcom {
        fn new(image: Vec<u8>) -> Self {
            FakeIcom {
                written: vec![0; image.len()],
                received: vec![false; image.len()],
                image,
                rev: MAP_REV,
                clone_in: false,
                ended: false,
                bad_checksum_at: None,
            }
        }

        fn reply(out: &mut Vec<u8>, cmd: u8, payload: &[u8]) {
            out.extend_from_slice(&[0xFE, 0xFE, RADIO, PC, cmd]);
            out.extend(escape(payload));
            out.push(0xFD);
        }
    }

    impl FakeRadio for FakeIcom {
        fn step(&mut self, req: &[u8], out: &mut Vec<u8>) -> usize {
            // Padding, and the stray FD the hispeed frame's payload carries.
            let lead = req.iter().take_while(|&&b| b == 0xFD).count();
            if lead > 0 {
                return lead;
            }
            let Some(end) = req.iter().position(|&b| b == 0xFD) else {
                return 0;
            };
            let f = &req[..=end];
            let fe = f.iter().take_while(|&&b| b == 0xFE).count();
            assert!(fe >= 2, "frame without FE FE: {f:02x?}");
            let body = &f[fe..f.len() - 1];
            assert_eq!(&body[..2], &[PC, RADIO], "frame not PC→radio");
            let (cmd, payload) = (body[2], &body[3..]);
            match cmd {
                CMD_ID => {
                    let mut md = MODEL.to_vec();
                    md.push(0x2D);
                    md.push(self.rev);
                    md.resize(49, 0x20);
                    Self::reply(out, CMD_MODEL, &md);
                }
                // The hispeed payload is 00 00 02 01 then its own FD, which
                // ended this frame early; the frame's real FD is consumed as a
                // stray on the next step.
                CMD_HISPEED => {}
                CMD_CLONE_OUT => {
                    for addr in (0..self.image.len()).step_by(32) {
                        let mut d = (addr as u32).to_be_bytes().to_vec();
                        d.push(32);
                        d.extend(self.image[addr..addr + 32].iter().map(|b| b ^ 0x80));
                        let mut cs = checksum(&d);
                        if self.bad_checksum_at == Some(addr) {
                            cs ^= 1;
                        }
                        d.push(cs);
                        Self::reply(out, CMD_DATA, &d);
                    }
                    Self::reply(out, CMD_END, ENDFRAME);
                }
                CMD_CLONE_IN => self.clone_in = true,
                CMD_DATA => {
                    assert!(self.clone_in, "data before clone-in");
                    let d = unescape(payload).unwrap();
                    let n = usize::from(d[4]);
                    assert_eq!(checksum(&d[..5 + n]), d[5 + n], "bad upload checksum");
                    let addr = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) as usize;
                    for (i, b) in d[5..5 + n].iter().enumerate() {
                        self.written[addr + i] = b ^ 0x80;
                        self.received[addr + i] = true;
                    }
                }
                CMD_END => {
                    assert_eq!(unescape(payload).unwrap(), ENDFRAME);
                    self.ended = true;
                    if self.clone_in {
                        Self::reply(out, CMD_OK, &[0x00]);
                    }
                }
                other => panic!("unexpected command {other:#04x}"),
            }
            end + 1
        }
    }

    /// A deterministic image with every byte value in it, so escaping (F A-F F)
    /// and the high-bit flip are both exercised.
    fn sample_image() -> Vec<u8> {
        (0..IMAGE_LEN).map(|i| (i * 7 + i / 256) as u8).collect()
    }

    #[test]
    fn escape_round_trips_every_byte() {
        let all: Vec<u8> = (0..=255).collect();
        let e = escape(&all);
        assert!(!e.contains(&0xFD) && !e.contains(&0xFE));
        assert_eq!(unescape(&e).unwrap(), all);
    }

    #[test]
    fn checksum_matches_chirps_formula() {
        // ((sum ^ 0xFFFF) + 1) & 0xFF == two's complement of the low byte.
        assert_eq!(checksum(&[0x00, 0x00, 0x01, 0x00, 0x20]), 0xDF);
        assert_eq!(checksum(&[]), 0x00);
    }

    #[test]
    fn identify_accepts_revision_3() {
        let mut p = FakePort::new(FakeIcom::new(sample_image()));
        let id = identify(&mut p).unwrap();
        assert_eq!(id.matched, "ID-5100");
        assert!(id.ident_hex.starts_with("348400012d03"));
    }

    #[test]
    fn identify_refuses_an_older_layout_by_name() {
        let mut radio = FakeIcom::new(sample_image());
        radio.rev = 2;
        let Err(e) = identify(&mut FakePort::new(radio)) else {
            panic!("a revision-2 radio was accepted");
        };
        assert!(e.contains("revision 2"), "{e}");
    }

    #[test]
    fn download_returns_the_radios_image() {
        let image = sample_image();
        let mut p = FakePort::new(FakeIcom::new(image.clone()));
        identify(&mut p).unwrap();
        assert_eq!(download(&mut p).unwrap(), image);
    }

    #[test]
    fn a_bad_frame_checksum_fails_the_download() {
        let mut radio = FakeIcom::new(sample_image());
        radio.bad_checksum_at = Some(0x1000);
        let mut p = FakePort::new(radio);
        identify(&mut p).unwrap();
        assert!(download(&mut p).unwrap_err().contains("0x01000"));
    }

    #[test]
    fn upload_delivers_every_byte_and_ends_confirmed() {
        let image = sample_image();
        let mut p = FakePort::new(FakeIcom::new(vec![0; IMAGE_LEN]));
        identify(&mut p).unwrap();
        upload(&mut p, &image).unwrap();
        assert!(p.radio.ended);
        assert!(p.radio.received.iter().all(|&r| r));
        assert_eq!(p.radio.written, image);
    }

    #[test]
    fn upload_refuses_a_wrong_length_image_before_sending() {
        let mut p = FakePort::new(FakeIcom::new(vec![0; IMAGE_LEN]));
        assert!(upload(&mut p, &[0u8; 100]).is_err());
        assert!(!p.radio.clone_in);
    }
}
