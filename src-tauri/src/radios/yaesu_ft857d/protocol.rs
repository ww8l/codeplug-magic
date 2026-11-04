//! The FT-857D / FT-897 clone, both directions.
//!
//! There is no handshake and the PC is never in charge. The operator puts the
//! radio in CLONE MODE (hold MODE◄ + MODE► while switching on) and then:
//!
//! - **read** — the app listens first, then the operator presses [C] SEND. The
//!   radio sends `[block#][data][sum8(data)]` and waits for `0x06` after each.
//!   Without an ACK it re-sends block 0 roughly every 6.6 s, then shows ERROR.
//! - **write** — the operator presses [A] RCV first, then the app sends the same
//!   frames and the radio ACKs each.
//!
//! Hardware status (2026-10-17, FT-897 US, Digirig): the **read** is proven —
//! three full reads, 53 blocks each, every sum good. The **write** follows
//! CHIRP's `_clone_out` and has never been run against a radio here; ladder
//! steps 1 and 2 are what prove it.
//!
//! Two read details came from the radio, not from CHIRP:
//! - Entering clone mode with the port already open can put a stray byte on the
//!   line before block 0 (`00 00 1F 06` was seen once). The reader slides past
//!   junk until block 0's frame checks out.
//! - Whether the image is US (one more 140-byte block, the 60 m channels) is
//!   decided by the radio sending that block or not. CHIRP makes the operator
//!   pick a model and errors on the wrong one; here the radio answers it.
//!
//! RTS and DTR are held low: the Digirig keys PTT on RTS, and both low is how
//! every successful read was taken.

use std::io::Write;
use std::time::{Duration, Instant};

use serialport::SerialPort;

use super::layout::{block_lengths, IMAGE_LEN_EXPORT, IMAGE_LEN_US, US_EXTRA_BLOCK};

pub(crate) const BAUD: u32 = 9600;
const ACK: u8 = 0x06;

/// How long to wait at each point. Tests shrink these; the radio does not.
#[derive(Clone, Copy)]
pub(crate) struct Patience {
    /// For block 0 of a read: the operator has to reach the radio and press SEND.
    pub first: Duration,
    /// For every later block, and for each ACK during a write.
    pub block: Duration,
    /// After the last export block, for the US radio's extra block.
    pub tail: Duration,
}

pub(crate) const PATIENCE: Patience = Patience {
    first: Duration::from_secs(120),
    block: Duration::from_secs(5),
    tail: Duration::from_secs(3),
};

/// What the operator does, worded once so the error messages and the dialog
/// cannot drift apart.
pub(crate) const CLONE_MODE_STEP: &str =
    "Turn the radio off, then hold MODE◄ and MODE► while switching it on until it shows CLONE MODE";
pub(crate) const SEND_STEP: &str = "press [C] (SEND) on the radio";
pub(crate) const RCV_STEP: &str = "press [A] (RCV) on the radio";

pub(crate) fn open_port(port: &str) -> Result<Box<dyn SerialPort>, String> {
    let mut p = serialport::new(port, BAUD)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        .timeout(Duration::from_millis(200))
        .open()
        .map_err(|e| format!("could not open {port}: {e}"))?;
    let _ = p.write_request_to_send(false);
    let _ = p.write_data_terminal_ready(false);
    Ok(p)
}

/// Read up to `n` bytes, giving up at `patience`. A short result is the
/// caller's to judge — the radio going quiet is an answer, not an I/O error.
fn read_n(p: &mut dyn SerialPort, n: usize, patience: Duration) -> Result<Vec<u8>, String> {
    let mut buf = Vec::with_capacity(n);
    let start = Instant::now();
    let mut chunk = [0u8; 512];
    while buf.len() < n && start.elapsed() < patience {
        let want = (n - buf.len()).min(chunk.len());
        match p.read(&mut chunk[..want]) {
            Ok(k) => buf.extend_from_slice(&chunk[..k]),
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(format!("serial read failed: {e}")),
        }
    }
    Ok(buf)
}

fn sum8(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |a, b| a.wrapping_add(*b))
}

/// Whether `frame` is block `n` of `len` data bytes with a good sum.
fn frame_ok(frame: &[u8], n: usize, len: usize) -> bool {
    frame.len() == len + 2 && frame[0] == n as u8 && sum8(&frame[1..=len]) == frame[len + 1]
}

/// Receive an image the operator sends with [C] SEND. Call this **before** the
/// operator presses SEND; it waits `patience.first` for block 0.
///
/// `progress(done, total)` is called after each block; `total` is the US count
/// since which kind of radio this is is not known until the end.
pub(crate) fn read_clone(
    p: &mut dyn SerialPort,
    patience: Patience,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Vec<u8>, String> {
    let blocks = block_lengths(false);
    let total = blocks.len() + 1;
    let mut image = Vec::with_capacity(IMAGE_LEN_US);
    for (n, &len) in blocks.iter().enumerate() {
        let mut frame =
            read_n(p, len + 2, if n == 0 { patience.first } else { patience.block })?;
        if n == 0 {
            if frame.is_empty() {
                return Err(format!(
                    "Nothing arrived from the radio. {CLONE_MODE_STEP}, then start the read and \
                     {SEND_STEP}."
                ));
            }
            // Slide past line junk until block 0 checks out.
            let deadline = Instant::now() + patience.first;
            while !frame_ok(&frame, 0, len) && frame.len() == len + 2 && Instant::now() < deadline {
                frame.remove(0);
                frame.extend(read_n(p, 1, patience.block)?);
            }
        }
        if !frame_ok(&frame, n, len) {
            return Err(if frame.len() < len + 2 {
                format!(
                    "The radio stopped sending at block {n} of {total} ({} of {} bytes). Check the \
                     cable and try again.",
                    frame.len(),
                    len + 2
                )
            } else {
                format!(
                    "Block {n} arrived damaged (header {:02X}, checksum {:02X} vs {:02X}).",
                    frame[0],
                    sum8(&frame[1..=len]),
                    frame[len + 1]
                )
            });
        }
        image.extend_from_slice(&frame[1..=len]);
        p.write_all(&[ACK]).map_err(|e| format!("serial write failed: {e}"))?;
        progress(n + 1, total);
    }

    // A US radio sends one more block; an export radio goes quiet.
    let n = blocks.len();
    let tail = read_n(p, US_EXTRA_BLOCK + 2, patience.tail)?;
    if !tail.is_empty() {
        if !frame_ok(&tail, n, US_EXTRA_BLOCK) {
            return Err(format!(
                "The radio sent {} bytes after the last expected block, not a block this app \
                 knows. Nothing was changed on the radio.",
                tail.len()
            ));
        }
        image.extend_from_slice(&tail[1..=US_EXTRA_BLOCK]);
        p.write_all(&[ACK]).map_err(|e| format!("serial write failed: {e}"))?;
        progress(total, total);
    }
    check_image(&image)?;
    Ok(image)
}

/// The image's shape: one of the two lengths the radio produces. Says nothing
/// about which unit it came from.
pub(crate) fn check_image(image: &[u8]) -> Result<(), String> {
    match image.len() {
        IMAGE_LEN_US | IMAGE_LEN_EXPORT => Ok(()),
        n => Err(format!(
            "{n} bytes is not an FT-857D/FT-897 clone image ({IMAGE_LEN_EXPORT} or {IMAGE_LEN_US})"
        )),
    }
}

/// Send `image` to a radio waiting in RCV. The operator presses [A] RCV
/// **before** this is called.
pub(crate) fn write_clone(
    p: &mut dyn SerialPort,
    image: &[u8],
    patience: Patience,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<(), String> {
    check_image(image)?;
    let blocks = block_lengths(image.len() == IMAGE_LEN_US);
    let total = blocks.len();
    let _ = p.clear(serialport::ClearBuffer::Input);
    let mut at = 0;
    for (n, &len) in blocks.iter().enumerate() {
        let data = &image[at..at + len];
        let mut frame = Vec::with_capacity(len + 2);
        frame.push(n as u8);
        frame.extend_from_slice(data);
        frame.push(sum8(data));
        // CHIRP pauses 10 ms before each block.
        std::thread::sleep(Duration::from_millis(10));
        p.write_all(&frame).map_err(|e| format!("serial write failed: {e}"))?;
        let reply = read_n(p, 1, patience.block)?;
        match reply.first() {
            Some(&ACK) => {}
            None if n == 0 => {
                return Err(format!(
                    "The radio did not answer. {CLONE_MODE_STEP}, {RCV_STEP}, and only then start \
                     the write. Nothing was written."
                ))
            }
            None => {
                return Err(format!(
                    "The radio stopped answering at block {n} of {total}. Its memory is now \
                     partly written."
                ))
            }
            Some(b) => {
                return Err(format!("The radio refused block {n} of {total} (replied {b:02X})."))
            }
        }
        at += len;
        progress(n + 1, total);
    }
    Ok(())
}

/// Identify over CAT, not clone: with the radio switched on normally, send the
/// read-only "read frequency and mode" command (`00 00 00 00 03`) at each of the
/// three CAT rates (menu 019) and report what answers. Proven on the FT-897
/// (2026-10-17: `01 42 35 65 0A` at 4800 = 14.235.65 MHz DIG, as displayed).
/// It cannot tell an FT-857D from an FT-897 — the command set is shared.
pub(crate) fn cat_identify(port: &str) -> Result<(u32, [u8; 5]), String> {
    for baud in [4800, 9600, 38400] {
        let mut p = serialport::new(port, baud)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::Two)
            .flow_control(serialport::FlowControl::None)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(|e| format!("could not open {port}: {e}"))?;
        let _ = p.write_request_to_send(false);
        let _ = p.write_data_terminal_ready(false);
        let _ = p.clear(serialport::ClearBuffer::All);
        p.write_all(&[0, 0, 0, 0, 0x03]).map_err(|e| format!("serial write failed: {e}"))?;
        let r = read_n(&mut *p, 5, Duration::from_millis(600))?;
        if let Ok(reply) = <[u8; 5]>::try_from(r.as_slice()) {
            return Ok((baud, reply));
        }
    }
    Err("No answer over CAT at 4800, 9600 or 38400. Switch the radio on normally (not in clone \
         mode), check menu 020 CAT/LIN/TUN is CAT, and that the cable is in the CAT/LINEAR jack."
        .to_string())
}

/// `01 42 35 65 0A` → "14.23565 MHz DIG".
pub(crate) fn cat_reply_text(r: &[u8; 5]) -> String {
    let digits: String = r[..4].iter().map(|b| format!("{b:02X}")).collect();
    let mhz = digits.parse::<u64>().map(|v| v as f64 / 100_000.0).unwrap_or(0.0);
    let mode = match r[4] {
        0x00 => "LSB",
        0x01 => "USB",
        0x02 => "CW",
        0x03 => "CWR",
        0x04 => "AM",
        0x06 => "WFM",
        0x08 => "FM",
        0x0A => "DIG",
        0x0C => "PKT",
        0x88 => "NFM",
        _ => "?",
    };
    format!("{mhz:.5} MHz {mode}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::fake_port::{FakePort, FakeRadio};

    const QUICK: Patience = Patience {
        first: Duration::from_millis(50),
        block: Duration::from_millis(50),
        tail: Duration::from_millis(20),
    };

    fn frame(n: usize, data: &[u8]) -> Vec<u8> {
        let mut f = vec![n as u8];
        f.extend_from_slice(data);
        f.push(sum8(data));
        f
    }

    /// A radio that has had SEND pressed: block 0 is already on the wire, and
    /// each ACK draws the next.
    struct Sender {
        frames: Vec<Vec<u8>>,
        next: usize,
    }

    impl Sender {
        fn port(image: &[u8], junk: &[u8]) -> FakePort<Sender> {
            let lens = block_lengths(image.len() == IMAGE_LEN_US);
            let mut frames = Vec::new();
            let mut at = 0;
            for (n, len) in lens.into_iter().enumerate() {
                frames.push(frame(n, &image[at..at + len]));
                at += len;
            }
            let mut first = junk.to_vec();
            first.extend_from_slice(&frames[0]);
            let mut p = FakePort::new(Sender { frames, next: 1 });
            p.radio_sends(&first);
            p
        }
    }

    impl FakeRadio for Sender {
        fn step(&mut self, req: &[u8], out: &mut Vec<u8>) -> usize {
            let Some(&b) = req.first() else { return 0 };
            assert_eq!(b, ACK, "the host only ever sends ACK during a read");
            if let Some(f) = self.frames.get(self.next) {
                out.extend_from_slice(f);
                self.next += 1;
            }
            1
        }
    }

    /// A radio that has had RCV pressed: it checks every frame and stores it.
    struct Receiver {
        lens: Vec<usize>,
        got: Vec<u8>,
        refuse: Option<usize>,
    }

    impl FakeRadio for Receiver {
        fn step(&mut self, req: &[u8], out: &mut Vec<u8>) -> usize {
            if req.is_empty() {
                return 0;
            }
            let n = self.got.len();
            let block = self.lens.iter().scan(0, |a, l| {
                let s = *a;
                *a += l;
                Some(s)
            });
            let idx = block.take_while(|&s| s < n).count();
            let Some(&len) = self.lens.get(idx) else { panic!("a frame past the last block") };
            if req.len() < len + 2 {
                return 0;
            }
            assert!(frame_ok(&req[..len + 2], idx, len), "frame {idx} malformed");
            if self.refuse == Some(idx) {
                out.push(0x15);
            } else {
                self.got.extend_from_slice(&req[1..=len]);
                out.push(ACK);
            }
            len + 2
        }
    }

    fn image(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 7 + 3) as u8).collect()
    }

    #[test]
    fn a_us_read_takes_all_53_blocks_and_skips_line_junk() {
        let want = image(IMAGE_LEN_US);
        // The stray 00 the FT-897 put on the line once.
        let mut p = Sender::port(&want, &[0x00]);
        let mut last = (0, 0);
        let got = read_clone(&mut p, QUICK, &mut |d, t| last = (d, t)).unwrap();
        assert_eq!(got, want);
        assert_eq!(last, (53, 53));
    }

    #[test]
    fn an_export_read_ends_quietly_after_52_blocks() {
        let want = image(IMAGE_LEN_EXPORT);
        let mut p = Sender::port(&want, &[]);
        assert_eq!(read_clone(&mut p, QUICK, &mut |_, _| {}).unwrap(), want);
    }

    #[test]
    fn a_silent_radio_is_told_how_to_send() {
        let mut p = FakePort::new(Sender { frames: vec![], next: 0 });
        let e = read_clone(&mut p, QUICK, &mut |_, _| {}).unwrap_err();
        assert!(e.contains("CLONE MODE") && e.contains("SEND"), "{e}");
    }

    #[test]
    fn a_write_sends_every_block_the_radio_checks() {
        for len in [IMAGE_LEN_US, IMAGE_LEN_EXPORT] {
            let img = image(len);
            let mut p = FakePort::new(Receiver {
                lens: block_lengths(len == IMAGE_LEN_US),
                got: vec![],
                refuse: None,
            });
            let mut last = (0, 0);
            write_clone(&mut p, &img, QUICK, &mut |d, t| last = (d, t)).unwrap();
            assert_eq!(p.radio.got, img);
            assert_eq!(last.0, last.1);
        }
    }

    #[test]
    fn a_refused_block_stops_the_write_and_says_where() {
        let img = image(IMAGE_LEN_US);
        let mut p = FakePort::new(Receiver {
            lens: block_lengths(true),
            got: vec![],
            refuse: Some(9),
        });
        let e = write_clone(&mut p, &img, QUICK, &mut |_, _| {}).unwrap_err();
        assert!(e.contains("block 9 of 53"), "{e}");
    }

    #[test]
    fn a_radio_not_in_rcv_is_told_how_and_nothing_was_written() {
        struct Deaf;
        impl FakeRadio for Deaf {
            fn step(&mut self, req: &[u8], _: &mut Vec<u8>) -> usize {
                req.len()
            }
        }
        let e = write_clone(&mut FakePort::new(Deaf), &image(IMAGE_LEN_US), QUICK, &mut |_, _| {})
            .unwrap_err();
        assert!(e.contains("RCV") && e.contains("Nothing was written"), "{e}");
    }

    #[test]
    fn the_cat_reply_the_ft897_gave_reads_as_its_display() {
        assert_eq!(cat_reply_text(&[0x01, 0x42, 0x35, 0x65, 0x0A]), "14.23565 MHz DIG");
    }

    #[test]
    fn a_wrong_length_image_is_never_sent() {
        let e = write_clone(&mut FakePort::new(Deaf2), &[0u8; 100], QUICK, &mut |_, _| {})
            .unwrap_err();
        assert!(e.contains("100 bytes"), "{e}");
        struct Deaf2;
        impl FakeRadio for Deaf2 {
            fn step(&mut self, _: &[u8], _: &mut Vec<u8>) -> usize {
                panic!("nothing should reach the radio")
            }
        }
    }
}
