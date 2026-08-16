//! Kenwood TH-D75 driver (`driver_key = "kenwood_thd75"`) — microSD programming.
//!
//! The third card radio, after the FT5D and the ID-52, and by some distance the
//! best-documented one this project has taken on. The radio saves its whole
//! configuration to
//! `KENWOOD/TH-D75/SETTINGS/DATA/MMDDYYYY_HHMMSS.d75` from **Menu >
//! Configuration > SD Card > Save Setting**, reads it back from **Load
//! Setting**, and mounts the card over USB-C so the card never has to come out.
//! [`d75`] is the container; [`memory`] writes the codeplug into it.
//!
//! ## Research first, and it paid
//!
//! Three published sources cover this format, and all three were checked against
//! a real file rather than taken on trust ([[research-before-reverse-engineering]]):
//!
//! | source | what it gives |
//! |---|---|
//! | CHIRP `chirp/drivers/thd74.py` | the entire channel memory layout — flags, 40-byte records, 16-character names, the 30 groups. `THD75Radio` is a bare subclass of `THD74Radio`. |
//! | `kitchen/th-d74-config-file` | the microSD file: the folder, the 0x100 header, "no checksum apparent", and the save→change→save→diff method |
//! | `LA3QMA/TH-D74-Kenwood` | ~50 serial commands and the enum tables — mode, shift, step, tone, DCS, D-STAR squelch |
//!
//! Decoding a real TH-D75 save with CHIRP's clone-image offsets applied at
//! `file[0x100..]` produced 91 memories that are unmistakably the operator's
//! own — Colorado repeaters with their real tones and shifts, GMRS, two airband
//! frequencies, five D-STAR hotspots — and the only occupied slots above 999 are
//! exactly CHIRP's WX and Call channel numbers. Compare the FT5D, where an
//! undocumented checksum cost four failed writes and a factory reset.
//!
//! ## What is claimed, and what is not
//!
//! Only [`CodeplugExporter`](crate::radios::driver::CodeplugExporter). The
//! settings read/write traits describe a **cable** session, and this driver has
//! no cable path: CHIRP's clone protocol for the D74 is real and implemented
//! upstream, but the card programs the radio completely and the operator has to
//! visit the radio's menus either way. A capability claimed before it works is a
//! button that fails on the radio.
//!
//! ## Verified on the radio (Phase 4)
//!
//! Both of the things this driver was guessing about have been measured, by
//! loading files into Tim's TH-D75 — see `scratchpad/thd75/FINDINGS.md` §11:
//!
//! 1. **There is no checksum.** A file whose body differed by 14 bytes, with
//!    nothing recomputed, loaded and showed the change. "No checksum apparent"
//!    was inherited from the D74 notes and is now measured — which matters,
//!    because it is exactly what was believed about the FT5D right up until an
//!    undocumented 32-bit checksum caused a factory reset.
//!    [[verify-hardware-claims-not-reports]]
//! 2. **The band code is about the band**, not the mode, so `memory::used_flag`
//!    is right. Every `7` in the sample was on a memory that was also AM, so the
//!    two were confounded; a probe memory at **52.525 MHz in FM** — Band-B-only
//!    and not AM — was accepted, along with all three 224 MHz repeaters the
//!    ID-52 silently threw away. [[radio-tx-vs-rx-bands]]
//!
//! Working notes: `scratchpad/thd75/FINDINGS.md` (gitignored — it references
//! dumps of a personal radio).

pub(crate) mod d75;
#[cfg(test)]
mod dev_export;
#[cfg(test)]
mod hw_phase4;
pub(crate) mod memory;
pub(crate) mod settings;

use crate::radios::driver::{RadioDriver, RadioIdentity};

pub(crate) struct KenwoodThd75;

pub(crate) static DRIVER: KenwoodThd75 = KenwoodThd75;

impl RadioDriver for KenwoodThd75 {
    fn key(&self) -> &'static str {
        "kenwood_thd75"
    }

    fn display_name(&self) -> &'static str {
        "Kenwood TH-D75"
    }

    /// Unused: nothing here opens a port. Kenwood's own figure for the D74/D75
    /// clone session is quoted so the number is not invented — CHIRP handshakes
    /// at 9600 and switches to 57600 for the transfer.
    fn baud(&self) -> u32 {
        9600
    }

    /// There is no handshake, because there is no cable path.
    ///
    /// Failing by name beats opening a port and timing out on a protocol nobody
    /// is speaking — the operator's next move is a menu on the radio, so the
    /// error says so.
    fn identify(&self, _port: &str) -> Result<RadioIdentity, String> {
        Err("The TH-D75 is programmed from its microSD card, not over a cable. Put the card in \
             this computer — or connect the radio over USB-C and switch it to SD card mode — and \
             use the card actions instead."
            .into())
    }

    fn as_codeplug_exporter(&self) -> Option<&dyn crate::radios::driver::CodeplugExporter> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::driver::DriverCapabilities;

    /// File export — the microSD codeplug — and nothing else. Every other flag
    /// means a cable session this driver has no protocol for. Locking the exact
    /// set catches drift in either direction: an unclaimed capability is an
    /// action the UI never offers, and a claimed one that does not work is an
    /// action that fails on the radio.
    #[test]
    fn advertises_file_export_only() {
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
                export: true,
                diagnostics: false,
            }
        );
    }

    /// A cable action must fail as an explanation, not as a serial timeout.
    #[test]
    fn identify_explains_the_card_path_instead_of_opening_a_port() {
        let Err(err) = DRIVER.identify("/dev/cu.nonesuch") else {
            panic!("identify claimed to reach a radio over a cable");
        };
        assert!(err.contains("microSD"), "{err}");
    }
}
