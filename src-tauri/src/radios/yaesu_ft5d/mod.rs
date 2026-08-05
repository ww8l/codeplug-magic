//! Yaesu FT5D driver (`driver_key = "yaesu_ft5d"`) — scaffolding + identify.
//!
//! This is the registry/model foundation for issue #32. No programming modality
//! is wired yet: the driver advertises no capabilities, so the generic dialog
//! offers the FT5D a connect handshake and nothing else. That is deliberate —
//! see "What is NOT here" below.
//!
//! ## The FT5D memory map IS the FT3D's (measured, s79)
//!
//! An earlier revision of this comment claimed the opposite — that memory
//! records lived at `0x1800`, flags at `0x12C0`, and that the FT5D was not
//! layout-compatible with the FT3D, so the family driver could not be lifted.
//! That came from a ~120-line RE harness in a personal fork
//! (sjlongland/chirp `fffd6ee`) and **it is wrong**. A real SD-card backup from
//! Tim's radio, decoded against an RT Systems export of the same codeplug,
//! says:
//!
//! | | claimed | measured |
//! |---|---|---|
//! | memory records | `0x1800` | **`0x2D40`** |
//! | flags | `0x12C0` | not a flag array; `0x12C0` is a second memory bank |
//! | FT3D-compatible | no | **yes** |
//!
//! The discrepancy is a coordinate-system error. `BACKUP.dat` is 130496 bytes;
//! the FT3D clone image is 130507. The difference is 11 = a 10-byte ident
//! header plus a 1-byte trailing checksum. CHIRP addresses are in *clone image*
//! coordinates, which include that header, and FT3D records sit at `0x2D4A` —
//! so `0x2D4A - 10 = 0x2D40`, exactly where the records are here.
//!
//! **The SD backup is the clone image with the header and checksum stripped**,
//! which makes CHIRP's `ft2d.py` a real porting base rather than a dead end.
//!
//! Mainline CHIRP still has no `ft5d.py`; the FT3D is an `FT2D` subclass with
//! `_model = b"AH72M"`. The **FT5D's token is `AH82M`**, read from `0x220` of
//! the backup — no cable required, which retires the main reason for the clone
//! probe below.
//!
//! ## Record layout (measured against known plaintext)
//!
//! Base `0x2D40`, stride 32, 999 slots (900 regular + 99 skip-search, matching
//! the published spec), unused slots `0xFF`-filled, record index = channel
//! number - 1. Verified by matching every frequency against the RT Systems
//! export: 199 of 200 exact.
//!
//! Decoded: offset direction (`0x01 & 0x30`), AM (`0x01 & 0x40`), RX frequency
//! (`0x02..0x05`, BCD kHz), tone mode (`0x05 & 0x03`), DN/C4FM (`0x05 & 0x20`),
//! TX power (`0x05 & 0xC0`), name (`0x08..0x18`, ASCII, `0xFF`-padded), repeater
//! offset (`0x18..0x1A`, BCD in 100 kHz units — and the Split TX frequency
//! overloads the same bytes), CTCSS (`0x1B`, standard 50-tone index), DCS
//! (`0x1C`), user CTCSS (`0x1D`), step (`0x1F & 0x10`).
//!
//! Still open: byte `0x00`'s exact semantics, `0x01`'s low nibble, `0x1F` bit 3,
//! skip and bank membership (both live outside the record), and the 500 Hz lost
//! to BCD truncation on 12.5 kHz-spaced 900 MHz channels.
//!
//! Full working notes, scripts and the byte-level tables are in
//! `scratchpad/ft5d/FINDINGS.md` (gitignored — raw dumps of a personal radio).
//! Per `no-inferred-addresses-on-hardware`, nothing above gets written to a
//! radio until the encoder round-trips against an RT Systems file first.
//!
//! ## The three programming modalities, none implemented yet
//!
//! 1. **ADMS-14 / RT Systems CSV** — a 53-column CSV. Needs no reverse
//!    engineering and no radio; the cheapest path to a usable export. Note that
//!    RT Systems doubles as a **reference encoder**: it can generate a valid
//!    FT5D SD file from any channel config with nothing plugged in, so our
//!    writer can be validated byte-for-byte offline before touching hardware.
//! 2. **microSD `FT5D/BACKUP/BACKUP.dat`** — cable-free, written by the radio's
//!    own Backup/Restore menu. Now largely decoded; see above.
//! 3. **USB clone mode** — SCU-19/39/57 cable, virtual COM port. Presumed to be
//!    the same radio-initiated `yaesu_clone` protocol as its siblings; see
//!    [`YaesuFt5d::identify`], which is the probe that will confirm it. Lower
//!    priority now that the SD path yielded both the model token and the layout.
//!
//! ## What is NOT here
//!
//! No [`ImageProgrammer`](crate::radios::driver::ImageProgrammer), no
//! [`SettingsReader`](crate::radios::driver::SettingsReader), no exporter. Every
//! `as_*` accessor stays at its `None` default, so
//! [`DriverCapabilities`](crate::radios::driver::DriverCapabilities) reports all
//! false and the UI cannot offer an action the driver cannot perform. Each
//! modality lands only once it has been proven against the radio.

/// Throwaway clone-port RE harness. Delete along with `hw_probe.rs` once the
/// clone protocol is settled — it is a measuring instrument, not driver code.
#[cfg(test)]
mod hw_probe;

use std::time::{Duration, Instant};

use serialport::{ClearBuffer, SerialPort};

use crate::radios::driver::{RadioDriver, RadioIdentity};

/// Clone-mode line rate shared by the FT1D/FT2D/FT3D family (`BAUD_RATE` in
/// CHIRP's `ft1d.py`). Assumed to carry over to the FT5D — unconfirmed.
const BAUD: u32 = 38400;

/// Per-read timeout. Doubles as the "radio has stopped sending" gap detector in
/// [`listen_for_ident`], so it must stay comfortably longer than the inter-byte
/// spacing of a 38400-baud stream (~0.26 ms).
const TIMEOUT: Duration = Duration::from_secs(1);

/// How long to wait for the FIRST byte. The Yaesu clone protocol is
/// radio-initiated: nothing arrives until the operator presses [Send] on the
/// radio, so this budget is human-paced, not protocol-paced.
const IDENT_WAIT: Duration = Duration::from_secs(30);

/// Upper bound on ident bytes collected. The FT1D/FT2D/FT3D ident block is 10
/// bytes (`_block_lengths[0]`); the cap is deliberately loose so a longer FT5D
/// block is captured in full rather than silently truncated to the family's
/// size — the whole point of the probe is to learn what the FT5D actually
/// sends.
const MAX_IDENT: usize = 64;

pub(crate) struct YaesuFt5d;

pub(crate) static DRIVER: YaesuFt5d = YaesuFt5d;

impl RadioDriver for YaesuFt5d {
    fn key(&self) -> &'static str {
        "yaesu_ft5d"
    }

    fn display_name(&self) -> &'static str {
        "Yaesu FT5D"
    }

    fn baud(&self) -> u32 {
        BAUD
    }

    /// Listen for the radio's clone-mode ident block.
    ///
    /// **This is a probe, not a settled protocol.** Every other driver here
    /// identifies by sending a magic string and matching the reply. The Yaesu
    /// clone protocol inverts that: the PC opens the port and waits, and the
    /// radio starts streaming when the operator presses [Send]. The first block
    /// it sends is the ident (CHIRP's `__clone_in` logs it as "ID block"), and
    /// the transfer only continues once the PC acks with `0x06`.
    ///
    /// **We deliberately never ack**, so no image transfer begins and nothing is
    /// read out of the radio beyond the ident bytes themselves — which is what
    /// makes this safe to run against an unknown protocol. Those bytes are the
    /// single most valuable unknown we have: they carry the FT5D's model token
    /// (the family's are `AH44M` / `AH60M` / `AH72M`), which is what a future
    /// clone-mode implementation must match on.
    ///
    /// Operator steps: radio off → cable to the DATA jack → hold [DISP] while
    /// powering on until "CLONE" shows → run this → press [Send] on the radio.
    fn identify(&self, port: &str) -> Result<RadioIdentity, String> {
        let mut p = open_port(port)?;
        let ident = listen_for_ident(&mut *p)?;
        Ok(RadioIdentity {
            matched: ascii(&ident),
            ident_hex: hex(&ident),
            ident_ascii: Some(ascii(&ident)),
        })
    }
}

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

/// Wait for the radio to start streaming, then collect its first block.
///
/// Two phases with different deadlines, because they are waiting on different
/// things: the first byte waits on a human pressing [Send] ([`IDENT_WAIT`]),
/// while the bytes after it arrive back-to-back and a single quiet
/// [`TIMEOUT`] means the block is complete.
fn listen_for_ident(p: &mut dyn SerialPort) -> Result<Vec<u8>, String> {
    let _ = p.clear(ClearBuffer::All);

    let deadline = Instant::now() + IDENT_WAIT;
    let first = loop {
        if let Some(b) = read_byte(p)? {
            break b;
        }
        if Instant::now() >= deadline {
            return Err(
                "no data from the radio. Put the FT5D in clone mode (hold [DISP] while \
                 powering on until \"CLONE\" shows), then press [Send] on the radio."
                    .into(),
            );
        }
    };

    let mut ident = vec![first];
    while ident.len() < MAX_IDENT {
        match read_byte(p)? {
            Some(b) => ident.push(b),
            // A full timeout with no byte: the radio has sent its block and is
            // waiting for the ack we are never going to send.
            None => break,
        }
    }
    Ok(ident)
}

/// Render bytes as space-separated uppercase hex.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::driver::DriverCapabilities;

    /// The scaffolding contract: the FT5D can be identified and nothing else.
    /// If a modality lands without its capability accessor, the UI silently
    /// keeps hiding it; if an accessor is added without a working
    /// implementation, the UI offers an action that fails on the radio. Lock
    /// the all-false state so either mistake is a test failure.
    #[test]
    fn advertises_no_capabilities_yet() {
        let caps = DriverCapabilities::of(&DRIVER);
        assert_eq!(
            caps,
            DriverCapabilities {
                program_image: false,
                read_settings: false,
                write_settings: false,
                write_channels: false,
                program_codeplug: false,
                write_callsign_db: false,
                export: false,
                diagnostics: false,
            }
        );
    }

    /// The ident bytes are the deliverable of the clone-mode probe, so both
    /// renderings must survive a block that is not all printable — the family's
    /// ident block is a model token padded with non-ASCII bytes.
    #[test]
    fn ident_renders_as_hex_and_printable_ascii() {
        let block = [0x41, 0x48, 0x37, 0x32, 0x4D, 0x00, 0xFF];
        assert_eq!(hex(&block), "41 48 37 32 4D 00 FF");
        assert_eq!(ascii(&block), "AH72M..");
    }
}
