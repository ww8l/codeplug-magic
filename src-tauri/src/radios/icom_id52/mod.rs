//! Icom ID-52 driver (`driver_key = "icom_id52"`) — microSD programming.
//!
//! Like the FT5D, this radio is programmed from its own memory card rather than
//! over a cable, and unlike the FT5D it offers **two independent card paths**,
//! which is exactly the channels/settings split issue #38 asks for:
//!
//! | what | file | radio menu |
//! |---|---|---|
//! | channels + groups | `ID-52/Csv/MemoryCh/*.csv` | SET > SD Card > Import/Export > Import > Memory CH |
//! | every MENU setting | `ID-52/Setting/*.icf` | SET > SD Card > Load Setting |
//!
//! The card does not even have to come out: **SET > Function > USB Connect >
//! SD Card Mode** mounts it over USB as mass storage.
//!
//! Neither path involves Icom's CS-52 programming software, which this project
//! does not depend on for any radio — the radio writes the files itself, and
//! reads them back itself.
//!
//! ## Status: the container is finished, the memory map is not
//!
//! [`icf`] is complete and tested: it parses a settings file, hands out the
//! memory image to patch, and re-emits it with a correct `#CD` MD5 — the
//! checksum that decides whether the radio accepts the file at all. That was
//! implementable ahead of any hardware because CHIRP's `icf.py` documents the
//! algorithm, which is a far better position than the FT5D's undocumented
//! 32-bit checksum (four failed writes and a factory reset before it fell).
//!
//! What is NOT known is where anything lives *inside* that image. CHIRP has no
//! ID-52 driver; the nearest relative, `id31.py`, describes 500 memories in 26
//! banks where the ID-52 has **1000 memories in 100 groups**, so its addresses
//! cannot carry over. They will be measured from a paired CSV export and `.icf`
//! taken off Tim's radio at the same moment — the radio's own export as known
//! plaintext, with no third-party software in the loop.
//!
//! Working notes, the CSV column table and the capture plan:
//! `scratchpad/id52/FINDINGS.md` and `CAPTURE-CHECKLIST.md` (gitignored — they
//! reference dumps of a personal radio).
//!
//! ## Why no capabilities are advertised yet
//!
//! The `as_*` accessors are what the UI reads to decide which actions to offer,
//! so a capability claimed before it works becomes a button that fails on the
//! radio. Nothing is claimed until it has been proven against one.

pub(crate) mod icf;

use crate::radios::driver::{RadioDriver, RadioIdentity};

pub(crate) struct IcomId52;

pub(crate) static DRIVER: IcomId52 = IcomId52;

impl RadioDriver for IcomId52 {
    fn key(&self) -> &'static str {
        "icom_id52"
    }

    fn display_name(&self) -> &'static str {
        "Icom ID-52"
    }

    /// Unused: this radio has no cable session to open. Icom's CI-V default for
    /// the family is quoted so the number is not invented, but nothing here
    /// opens a port.
    fn baud(&self) -> u32 {
        19200
    }

    /// There is no handshake, because there is no cable path.
    ///
    /// The ID-52 can clone over USB to Icom's CS-52 software, but that protocol
    /// is not implemented and would not be worth implementing: the card path
    /// programs the radio completely, and the operator already has to visit the
    /// radio's menus either way. Failing by name here beats opening a port and
    /// timing out on a protocol nobody is speaking.
    fn identify(&self, _port: &str) -> Result<RadioIdentity, String> {
        Err("The ID-52 is programmed from its microSD card, not over a cable. \
             Put the card in this computer — or connect the radio with SET > Function > \
             USB Connect > SD Card Mode — and use the card actions instead."
            .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::driver::DriverCapabilities;

    /// The ID-52 claims nothing yet. When the card paths land, this test is the
    /// thing that forces the capability set to be updated deliberately rather
    /// than drifting — in either direction: an unclaimed capability is an
    /// action the UI never offers, and a claimed one that does not work is an
    /// action that fails on the radio.
    #[test]
    fn advertises_nothing_until_a_path_is_proven() {
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

    /// A cable action must fail as an explanation, not as a serial timeout —
    /// the operator's next move is a menu on the radio, so the error has to say
    /// so.
    #[test]
    fn identify_explains_the_card_path_instead_of_opening_a_port() {
        let Err(err) = DRIVER.identify("/dev/cu.nonesuch") else {
            panic!("identify claimed to reach a radio over a cable");
        };
        assert!(err.contains("microSD"), "{err}");
    }
}
