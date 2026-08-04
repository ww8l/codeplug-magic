//! Yaesu FT5D driver (`driver_key = "yaesu_ft5d"`) — scaffolding + identify.
//!
//! This is the registry/model foundation for issue #32. No programming modality
//! is wired yet: the driver advertises no capabilities, so the generic dialog
//! offers the FT5D a connect handshake and nothing else. That is deliberate —
//! see "What is NOT here" below.
//!
//! ## Why there is no CHIRP driver to port
//!
//! Unlike our other three radios, the FT5D has NO upstream CHIRP driver.
//! Mainline CHIRP covers the FT1D/FT2D/FT3D family (`chirp/drivers/ft1d.py`,
//! `ft2d.py`; the FT3D is an `FT2D` subclass with `_model = b"AH72M"`), but
//! there is no `ft5d.py`. The only public FT5D code is a ~120-line
//! reverse-engineering harness in a personal fork (sjlongland/chirp commit
//! `fffd6ee`) that parses a `MEMORY.dat` and prints channels.
//!
//! **The FT5D is not layout-compatible with the FT3D**, so the existing family
//! driver cannot be lifted:
//!
//! | | FT1D/FT2D/FT3D | FT5D |
//! |---|---|---|
//! | channel flags | inside the 0x047E block set | `0x12C0` |
//! | memory records | `0x2D4A` | `0x1800` |
//! | image size     | 130507 (`_memsize`) | unknown |
//!
//! ## What is known about the memory layout (UNVERIFIED — no dump seen yet)
//!
//! From the harness plus a hexdump writeup by the same author (vk4msl.com):
//! a 1-byte-per-channel flag array at `0x12C0` (`valid`/`used`/`skip`/`pskip`),
//! then 999 × 32-byte memory records at `0x1800`. Each record holds a 2-bit
//! band selector, a 3-byte BCD frequency in kHz, a 3-bit mode (FM / AMS C4FM /
//! C4FM), a 3-bit squelch type, a CTCSS tone index into a 49-entry table, and a
//! 16-char label. **12 of the 32 bytes are still unknown, and TX offset, duplex
//! direction, DCS code, power level, step and the skip flags are all inside
//! them** — so no repeater channel can be encoded from public knowledge today.
//!
//! 999 records lines up with the published spec (900 regular memories + 99
//! skip-search memories), which is the one independent check we have on the
//! layout. It is still an inference, not a measurement — see the
//! `no-inferred-addresses-on-hardware` rule: nothing here gets written to flash
//! until it is proven against a real dump.
//!
//! ## The three programming modalities, none implemented yet
//!
//! 1. **ADMS-14 CSV** — a headerless 53-column CSV the Yaesu/RT Systems tools
//!    import. Fully documented and needs no reverse engineering; this is the
//!    cheapest path to a usable export.
//! 2. **microSD `MEMORY.dat`** — cable-free, written by the radio's own
//!    Backup/Restore menu. Needs the RE work described above.
//! 3. **USB clone mode** — SCU-19/39/57 cable, virtual COM port. Presumed to be
//!    the same radio-initiated `yaesu_clone` protocol as its siblings; see
//!    [`YaesuFt5d::identify`], which is the probe that will confirm it.
//!
//! ## What is NOT here
//!
//! No [`ImageProgrammer`](crate::radios::driver::ImageProgrammer), no
//! [`SettingsReader`](crate::radios::driver::SettingsReader), no exporter. Every
//! `as_*` accessor stays at its `None` default, so
//! [`DriverCapabilities`](crate::radios::driver::DriverCapabilities) reports all
//! false and the UI cannot offer an action the driver cannot perform. Each
//! modality lands only once it has been proven against the radio.

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
