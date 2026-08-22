//! A `SerialPort` with a radio behind it, for tests (#89).
//!
//! Every function below `open_port` in every driver takes `&mut dyn SerialPort`
//! — they are written to be faked — but nothing was faking them, so the
//! read → plan → write → commit sequence had never been executed outside a
//! session with a real radio on the cable. That is the only code in this repo
//! that can leave a radio half-written.
//!
//! [`FakePort`] is the transport: it buffers what the host writes, hands the
//! whole of it to a [`FakeRadio`], and queues whatever the radio replies. The
//! radio is a separate trait so one transport serves every driver — the AnyTone
//! model lives here as [`FakeAnytone`]; a UV-5R or TD-H3 model is the same
//! shape.
//!
//! ⚠ What a fake CANNOT do: prove the wire format is right. It replays the
//! format this repo believes in, so a driver and its fake are wrong together if
//! the belief is wrong. It proves the SEQUENCING — which addresses are read,
//! what is written back, whether END is sent, what happens when a frame is
//! refused — which is the half that no hardware session ever checked either,
//! because a real radio cannot be asked to refuse frame 37 on demand.

use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Result as IoResult, Write};
use std::time::Duration;

use serialport::{
    ClearBuffer, DataBits, FlowControl, Parity, Result as SpResult, SerialPort, StopBits,
};

/// A radio at the far end of the cable.
pub(crate) trait FakeRadio: Send {
    /// Consume as much of `req` as forms one complete command, appending the
    /// reply to `out`. Returns bytes consumed; `0` means "incomplete, wait for
    /// more". Implementations should panic on a byte they cannot classify —
    /// a framing desync is a bug in the code under test, not a quiet no-op.
    fn step(&mut self, req: &[u8], out: &mut Vec<u8>) -> usize;
}

/// The transport half: buffers, framing, and the `SerialPort` surface.
pub(crate) struct FakePort<R: FakeRadio> {
    pub radio: R,
    /// Bytes the host has written that do not yet form a whole command.
    pending: Vec<u8>,
    /// Bytes the radio has replied that the host has not read yet.
    inbox: VecDeque<u8>,
    timeout: Duration,
}

impl<R: FakeRadio> FakePort<R> {
    pub fn new(radio: R) -> Self {
        Self {
            radio,
            pending: Vec::new(),
            inbox: VecDeque::new(),
            timeout: Duration::from_millis(1),
        }
    }

    fn pump(&mut self) {
        loop {
            let mut reply = Vec::new();
            let used = self.radio.step(&self.pending, &mut reply);
            self.inbox.extend(reply);
            if used == 0 {
                break;
            }
            self.pending.drain(..used);
        }
    }
}

impl<R: FakeRadio> Read for FakePort<R> {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        let n = buf.len().min(self.inbox.len());
        for slot in buf.iter_mut().take(n) {
            *slot = self.inbox.pop_front().expect("checked len");
        }
        // `read_byte` reads Ok(0) as "the radio went quiet", which is exactly
        // what an empty inbox means. No error, no blocking, no sleeping.
        Ok(n)
    }
}

impl<R: FakeRadio> Write for FakePort<R> {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        self.pending.extend_from_slice(buf);
        self.pump();
        Ok(buf.len())
    }
    fn flush(&mut self) -> IoResult<()> {
        Ok(())
    }
}

/// Everything here is either a fixed answer or a no-op: none of it is part of
/// the clone protocol, and a driver that starts depending on one should say so
/// out loud rather than silently getting a plausible default.
impl<R: FakeRadio> SerialPort for FakePort<R> {
    fn name(&self) -> Option<String> {
        Some("fake".into())
    }
    fn baud_rate(&self) -> SpResult<u32> {
        Ok(115_200)
    }
    fn data_bits(&self) -> SpResult<DataBits> {
        Ok(DataBits::Eight)
    }
    fn flow_control(&self) -> SpResult<FlowControl> {
        Ok(FlowControl::None)
    }
    fn parity(&self) -> SpResult<Parity> {
        Ok(Parity::None)
    }
    fn stop_bits(&self) -> SpResult<StopBits> {
        Ok(StopBits::One)
    }
    fn timeout(&self) -> Duration {
        self.timeout
    }
    fn set_baud_rate(&mut self, _: u32) -> SpResult<()> {
        Ok(())
    }
    fn set_data_bits(&mut self, _: DataBits) -> SpResult<()> {
        Ok(())
    }
    fn set_flow_control(&mut self, _: FlowControl) -> SpResult<()> {
        Ok(())
    }
    fn set_parity(&mut self, _: Parity) -> SpResult<()> {
        Ok(())
    }
    fn set_stop_bits(&mut self, _: StopBits) -> SpResult<()> {
        Ok(())
    }
    fn set_timeout(&mut self, t: Duration) -> SpResult<()> {
        self.timeout = t;
        Ok(())
    }
    fn write_request_to_send(&mut self, _: bool) -> SpResult<()> {
        Ok(())
    }
    fn write_data_terminal_ready(&mut self, _: bool) -> SpResult<()> {
        Ok(())
    }
    fn read_clear_to_send(&mut self) -> SpResult<bool> {
        Ok(true)
    }
    fn read_data_set_ready(&mut self) -> SpResult<bool> {
        Ok(true)
    }
    fn read_ring_indicator(&mut self) -> SpResult<bool> {
        Ok(false)
    }
    fn read_carrier_detect(&mut self) -> SpResult<bool> {
        Ok(true)
    }
    fn bytes_to_read(&self) -> SpResult<u32> {
        Ok(self.inbox.len() as u32)
    }
    fn bytes_to_write(&self) -> SpResult<u32> {
        Ok(0)
    }
    /// ⚠ Deliberately does NOT drop the inbox. `enter_program_and_ident` calls
    /// this before every handshake attempt; a real port drops bytes still in
    /// flight, but dropping a reply the host has not read yet would make the
    /// fake lose frames the code under test is entitled to. Nothing in these
    /// drivers depends on `clear` discarding anything.
    fn clear(&self, _: ClearBuffer) -> SpResult<()> {
        Ok(())
    }
    fn try_clone(&self) -> SpResult<Box<dyn SerialPort>> {
        Err(serialport::Error::new(
            serialport::ErrorKind::NoDevice,
            "a FakePort cannot be cloned — the radio model has one owner",
        ))
    }
    fn set_break(&self) -> SpResult<()> {
        Ok(())
    }
    fn clear_break(&self) -> SpResult<()> {
        Ok(())
    }
}

// ============================================================
// AnyTone D890UV
// ============================================================

use super::anytone_atd890uv::{anytone_checksum, ACK, END, PROGRAM, PROGRAM_ACK};

/// A D890UV's clone-mode side of the conversation, over a sparse flash model.
///
/// Flash defaults to `0xFF` (erased), so a test only has to seed the bytes it
/// cares about — and a read of an address nobody seeded comes back as the
/// radio's own empty pattern, which is what makes the "stop at the first empty
/// bank" logic testable.
pub(crate) struct FakeAnytone {
    flash: BTreeMap<u32, u8>,
    ident: Vec<u8>,
    /// Every write frame accepted, in order: `(address, data)`.
    pub writes: Vec<(u32, Vec<u8>)>,
    /// Every address read, in order.
    pub reads: Vec<u32>,
    /// True once `END` has been acknowledged.
    pub ended: bool,
    /// Refuse (stay silent on) the write frame at this address, once.
    pub refuse_write_at: Option<u32>,
    /// Reply to the first `PROGRAM` with silence this many times.
    pub ignore_program: usize,
}

impl FakeAnytone {
    pub fn new() -> Self {
        Self {
            flash: BTreeMap::new(),
            ident: b"ID890UV".to_vec(),
            writes: Vec::new(),
            reads: Vec::new(),
            ended: false,
            refuse_write_at: None,
            ignore_program: 0,
        }
    }

    /// Answer the ident handshake as a different AnyTone — the D878 that shares
    /// this protocol and must not be programmed with D890 content.
    pub fn identifying_as(mut self, ident: &[u8]) -> Self {
        self.ident = ident.to_vec();
        self
    }

    pub fn seed(&mut self, addr: u32, data: &[u8]) {
        for (i, &b) in data.iter().enumerate() {
            self.flash.insert(addr + i as u32, b);
        }
    }

    pub fn peek(&self, addr: u32, len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| self.byte(addr + i as u32))
            .collect()
    }

    fn byte(&self, addr: u32) -> u8 {
        self.flash.get(&addr).copied().unwrap_or(0xFF)
    }
}

impl FakeRadio for FakeAnytone {
    fn step(&mut self, req: &[u8], out: &mut Vec<u8>) -> usize {
        let Some(&first) = req.first() else {
            return 0;
        };
        match first {
            // PROGRAM
            b'P' => {
                if req.len() < PROGRAM.len() {
                    return 0;
                }
                assert_eq!(&req[..PROGRAM.len()], PROGRAM, "malformed PROGRAM");
                if self.ignore_program > 0 {
                    self.ignore_program -= 1;
                } else {
                    out.extend_from_slice(PROGRAM_ACK);
                }
                PROGRAM.len()
            }
            // Identify
            0x02 => {
                out.extend_from_slice(&self.ident);
                out.push(ACK);
                1
            }
            // END
            b'E' => {
                if req.len() < END.len() {
                    return 0;
                }
                assert_eq!(&req[..END.len()], END, "malformed END");
                self.ended = true;
                out.push(ACK);
                END.len()
            }
            // Read: R + addr(4 BE) + size(1)
            b'R' => {
                if req.len() < 6 {
                    return 0;
                }
                let addr = u32::from_be_bytes([req[1], req[2], req[3], req[4]]);
                let size = req[5];
                self.reads.push(addr);
                let data = self.peek(addr, size as usize);
                out.push(b'W');
                out.extend_from_slice(&addr.to_be_bytes());
                out.push(size);
                out.extend_from_slice(&data);
                out.push(anytone_checksum(&addr.to_be_bytes(), &data));
                out.push(ACK);
                6
            }
            // Write: W + addr(4 BE) + len(1) + data + checksum(1) + 0x06
            b'W' => {
                if req.len() < 6 {
                    return 0;
                }
                let addr = u32::from_be_bytes([req[1], req[2], req[3], req[4]]);
                let len = req[5] as usize;
                let total = 6 + len + 2;
                if req.len() < total {
                    return 0;
                }
                let data = req[6..6 + len].to_vec();
                assert_eq!(
                    req[6 + len],
                    anytone_checksum(&addr.to_be_bytes(), &data),
                    "bad write checksum at {addr:#010X}"
                );
                assert_eq!(req[6 + len + 1], ACK, "write frame missing its 0x06 terminator");
                if self.refuse_write_at == Some(addr) {
                    // Silence: the radio rejected the frame. `write_block` must
                    // stop here rather than carry on writing.
                    self.refuse_write_at = None;
                } else {
                    for (i, &b) in data.iter().enumerate() {
                        self.flash.insert(addr + i as u32, b);
                    }
                    self.writes.push((addr, data));
                    out.push(ACK);
                }
                total
            }
            other => panic!(
                "the driver sent {other:#04X} where a command byte was expected — framing desync"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::anytone_atd890uv::{
        end_session, enter_program_and_ident, read_block, write_block, CHANNEL_BASE, READ_FRAME,
        WRITE_WINDOW,
    };

    fn port() -> FakePort<FakeAnytone> {
        FakePort::new(FakeAnytone::new())
    }

    /// The handshake, end to end: PROGRAM → QX+ack → 0x02 → ident.
    #[test]
    fn the_handshake_completes_and_the_ident_reaches_the_caller() {
        let mut p = port();
        let ident = enter_program_and_ident(&mut p).expect("handshake");
        assert_eq!(ident, b"ID890UV");
    }

    /// A D878 answers this same handshake — the whole family shares it — and
    /// must be refused before any write path can reach `write_block`.
    #[test]
    fn a_sibling_radio_on_the_port_is_refused_by_the_ident() {
        let mut p = FakePort::new(FakeAnytone::new().identifying_as(b"ID878UV"));
        let err = enter_program_and_ident(&mut p).unwrap_err();
        assert!(err.contains("878") || err.contains("not a D890UV"), "{err}");
    }

    /// The retry loop exists because AnyTone radios routinely ignore the first
    /// PROGRAM after a previous session. Nothing had ever exercised it.
    #[test]
    fn a_radio_that_ignores_the_first_program_is_retried_not_failed() {
        let mut radio = FakeAnytone::new();
        radio.ignore_program = 2;
        let mut p = FakePort::new(radio);
        assert_eq!(enter_program_and_ident(&mut p).expect("handshake"), b"ID890UV");
    }

    /// Reads are checksummed per frame and stitched back into one block, and
    /// unseeded flash reads as the radio's own erased pattern.
    #[test]
    fn a_block_read_stitches_frames_and_verifies_every_checksum() {
        let mut radio = FakeAnytone::new();
        let seeded: Vec<u8> = (0..64u32).map(|i| (i * 7) as u8).collect();
        radio.seed(CHANNEL_BASE, &seeded);
        let mut p = FakePort::new(radio);

        enter_program_and_ident(&mut p).expect("handshake");
        let got = read_block(&mut p, CHANNEL_BASE, 96).expect("read");
        assert_eq!(&got[..64], &seeded[..]);
        assert!(got[64..].iter().all(|&b| b == 0xFF), "unseeded flash reads erased");

        // One request per READ_FRAME, at ascending addresses — the contiguous
        // sweep the radio streamed on a real read.
        let want: Vec<u32> = (0..6).map(|i| CHANNEL_BASE + i * READ_FRAME as u32).collect();
        assert_eq!(p.radio.reads, want);
    }

    /// A write lands in flash, frame by frame, and reads back as itself.
    #[test]
    fn a_block_write_lands_and_reads_back() {
        let mut p = port();
        enter_program_and_ident(&mut p).expect("handshake");

        let payload: Vec<u8> = (0..WRITE_WINDOW).map(|i| (i ^ 0xA5) as u8).collect();
        write_block(&mut p, CHANNEL_BASE, &payload).expect("write");
        assert_eq!(
            p.radio.writes.len(),
            WRITE_WINDOW / READ_FRAME as usize,
            "one frame per READ_FRAME bytes"
        );
        assert_eq!(p.radio.peek(CHANNEL_BASE, WRITE_WINDOW), payload);

        end_session(&mut p).expect("end");
        assert!(p.radio.ended);
    }

    /// ★ The path that can leave a radio half-written, and the one a real radio
    /// cannot be asked to produce on demand: a frame the radio refuses.
    ///
    /// `write_block` must STOP at that frame — not carry on and not retry — and
    /// the error must be the one that tells the operator to restore. Everything
    /// before the refused frame is already in flash, which is exactly why the
    /// message has to say so.
    #[test]
    fn a_refused_frame_stops_the_write_and_says_to_restore() {
        let mut radio = FakeAnytone::new();
        let stop_at = CHANNEL_BASE + 3 * READ_FRAME as u32;
        radio.refuse_write_at = Some(stop_at);
        let mut p = FakePort::new(radio);
        enter_program_and_ident(&mut p).expect("handshake");

        let payload = vec![0x11u8; WRITE_WINDOW];
        let err = write_block(&mut p, CHANNEL_BASE, &payload).unwrap_err();

        assert!(err.contains(&format!("{stop_at:08X}")), "{err}");
        assert!(err.contains("STOP and restore from the backup"), "{err}");
        // Stopped: only the three frames before it were accepted.
        assert_eq!(p.radio.writes.len(), 3);
        assert_eq!(
            p.radio.writes.last().map(|(a, _)| *a),
            Some(CHANNEL_BASE + 2 * READ_FRAME as u32)
        );
        // And the session was NOT ended — ending would commit the half-written
        // image, which is why `run_program` deliberately leaves it open.
        assert!(!p.radio.ended);
    }
}
