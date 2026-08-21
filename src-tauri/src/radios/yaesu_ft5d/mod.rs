//! Yaesu FT5D driver (`driver_key = "yaesu_ft5d"`) — identify + microSD codeplug
//! writing.
//!
//! The programming path is the **microSD card**, in [`sd_image`]: the operator
//! backs the radio up to its card, we patch memories, flags and banks into that
//! `BACKUP.dat`, and the radio's own Restore menu reads it back. No cable, and
//! no third-party software. See that module for the byte layout.
//!
//! [`settings`] rides the same file: the radio profile's 67 non-channel
//! settings are read out of a backup and written back into it, so the FT5D gets
//! a populated profile form without a cable modality existing.
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
//! ## The decode is complete, and it is CHIRP's struct
//!
//! Every field was measured against known plaintext (a real backup paired with
//! an RT Systems export of the same codeplug), then cross-checked against
//! CHIRP's `ft1d.py` `MEM_FORMAT`, which lines up address-for-address under the
//! −10 rule: `bank_info 0x0EFE→0x0EF4`, `bank_members 0x154A→0x1540`,
//! `flag 0x2800`, `memory 0x2D4A→0x2D40`. A decoder written from those constants
//! reproduces all 53 CSV columns for all 200 channels with **zero mismatches**,
//! so [`sd_image`] holds the layout and this comment does not repeat it.
//!
//! Two corrections to earlier revisions of this comment, both of which would
//! have produced a broken write:
//!
//! - **900 memory slots, not 999.** The extra 99 are `Skip[]`, a separate
//!   skip-search array with its own flags — not extra channel space.
//! - **Unused slots are NOT reliably `0xFF`-filled.** `flag[900]` at `0x2800` is
//!   what makes a channel exist, exactly like the AnyTone present-bitmaps. The
//!   real dump held 201 non-`0xFF` records for 200 channels: slot 30 was a stale
//!   leftover the flag array correctly called empty.
//!
//! Working notes and byte tables: `scratchpad/ft5d/FINDINGS.md` (gitignored —
//! raw dumps of a personal radio).
//!
//! ## The other two modalities, still not implemented
//!
//! 1. **ADMS-14 / RT Systems CSV** — deliberately not built. RT Systems was a
//!    diagnostic instrument for the decode (its export was the known plaintext),
//!    not something the app should depend on or emit.
//! 2. **USB clone mode** — SCU-19/39/57 cable, virtual COM port. Presumed to be
//!    the same radio-initiated `yaesu_clone` protocol as its siblings; see
//!    [`YaesuFt5d::identify`], which is the probe that would confirm it. Low
//!    priority: the SD path programs the radio without a cable at all.
//!
//! ## What is NOT here
//!
//! No [`ImageProgrammer`](crate::radios::driver::ImageProgrammer) and no
//! [`SettingsReader`](crate::radios::driver::SettingsReader) — both mean live
//! USB, which is the unproven modality. `export` is the only capability the
//! driver claims, so the UI cannot offer an action it cannot perform.
//!
//! Note that the FT5D's settings therefore do NOT go through the capability
//! flags: `read_settings` stays false and the profile editor reaches
//! [`settings::decode_settings`] through the file-based
//! `read_ft5d_settings_from_backup` command instead. The flags describe what
//! the driver can do *over a cable*, and the answer is still "identify only".

/// Throwaway clone-port RE harness. Delete along with `hw_probe.rs` once the
/// clone protocol is settled — it is a measuring instrument, not driver code.
#[cfg(test)]
mod hw_probe;

/// Checks whether the USB-serial adapter really honours a baud-rate change,
/// which `SerialPort::baud_rate()` cannot tell you. Throwaway, like `hw_probe`.
#[cfg(test)]
mod baud_reality;

pub(crate) mod sd_image;
pub(crate) mod settings;

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
    /// Operator steps: radio off → cable to the DATA jack → hold [F] while
    /// powering on → the screen shows CLONE with two touch buttons, RECEIVE and
    /// SEND → run this → tap SEND.
    ///
    /// Photographed on the radio (s85). CHIRP's FT1D key names ([BAND] to send,
    /// [Dx] to receive) do not apply: the FT5D is touchscreen and has no [Dx]
    /// key at all.
    fn identify(&self, port: &str) -> Result<RadioIdentity, String> {
        let mut p = open_port(port)?;
        let ident = listen_for_ident(&mut *p)?;
        Ok(RadioIdentity {
            matched: ascii(&ident),
            ident_hex: hex(&ident),
            ident_ascii: Some(ascii(&ident)),
        })
    }

    fn as_codeplug_exporter(&self) -> Option<&dyn crate::radios::driver::CodeplugExporter> {
        Some(self)
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
                "no data from the radio. Put the FT5D in clone mode (hold [F] while \
                 powering on until \"CLONE\" shows), then tap SEND on the screen."
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

    /// The FT5D's contract: file export (the microSD codeplug) and nothing
    /// else. If a modality lands without its capability accessor, the UI
    /// silently keeps hiding it; if an accessor is added without a working
    /// implementation, the UI offers an action that fails on the radio. Lock
    /// the exact set so either mistake is a test failure.
    #[test]
    fn advertises_export_only() {
        let caps = DriverCapabilities::of(&DRIVER);
        assert_eq!(
            caps,
            DriverCapabilities {
                program_image: false,
                restore_image: false,
                read_settings: false,
                write_settings: false,
                write_channels: false,
                program_codeplug: false,
                write_callsign_db: false,
                export: true,
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
