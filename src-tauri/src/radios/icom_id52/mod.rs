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
//! ## One file, not two
//!
//! The table above is how the radio presents it, but the `.icf` is the better
//! path and is the one the app programs from: `Load Setting` offers **ALL**,
//! which restores "all Memory channels, settings on the MENU screen, and the
//! Repeater List" in a single operation. The Memory CH CSV stays as a plain
//! export — useful on its own, and useful as a cross-check, since the same
//! codeplug down both paths should describe the same radio.
//!
//! ## What is built
//!
//! [`icf`] is complete and tested: it parses a settings file, hands out the
//! memory image to patch, and re-emits it with a correct `#CD` MD5 — the
//! checksum that decides whether the radio accepts the file at all. That was
//! implementable ahead of any hardware because CHIRP's `icf.py` documents the
//! algorithm, which is a far better position than the FT5D's undocumented
//! 32-bit checksum (four failed writes and a factory reset before it fell).
//!
//! [`memory`] writes the memory pool into that image: records, the skip bitmap,
//! the group table, the chains and the channel-number maps. Every address in it
//! was measured off Tim's radio, none inferred from CHIRP's `id31.py` — that
//! driver describes 500 memories in 26 banks where this radio has **1000 in 100
//! groups**, so its addresses could not carry over even as a starting point.
//! The check that matters: re-encoding the radio's own 80 memories from its own
//! CSV export reproduces the bytes the radio wrote, exactly, in every field the
//! radio reads.
//!
//! [`settings`] does the same for the MENU settings — 160 of them, every offset
//! measured by diffing two real saves, and the same file carries them, which is
//! what makes `Load Setting > ALL` a complete programming operation.
//!
//! Working notes, the CSV column table and the capture plan:
//! `scratchpad/id52/FINDINGS.md`, `CAPTURE-CHECKLIST.md` and
//! `PASS3-SHEET.md` (gitignored — they reference dumps of a personal radio).
//!
//! ## What is claimed, and what is not
//!
//! The `as_*` accessors are what the UI reads to decide which actions to offer,
//! so a capability claimed before it works becomes a button that fails on the
//! radio. Only [`CodeplugExporter`](crate::radios::driver::CodeplugExporter) is
//! claimed, and that is the whole card path: the exporter patches the operator's
//! own `.icf` (or writes the Memory CH CSV, by extension). The settings
//! read/write traits stay unimplemented on purpose — they describe a *cable*
//! settings session, which this radio has no protocol for; its settings are read
//! by the dedicated `read_id52_settings_from_card` command and written by the
//! same export that writes the channels, exactly as the FT5D does.
//!
//! ⚠ Still unproven on hardware: nothing written by this driver has been loaded
//! into a radio yet. And one class of value is known to be suspect — the enums
//! were measured through RT Systems, which saves `#MapRev=1` files while the
//! radio itself writes revision 3. Addresses have been identical across both all
//! along; option lists that *grew* between revisions have not been checked.

#[cfg(test)]
mod dump_card;
pub(crate) mod icf;
pub(crate) mod memory;
pub(crate) mod memory_csv;
pub(crate) mod settings;

use crate::radios::driver::{RadioDriver, RadioIdentity};

/// Line 1 of an ID-52 `.icf`. Measured from a real export off the radio (s88);
/// the trailing `0001` is part of the id, not padding, though only the first
/// two bytes feed the checksum magic.
pub(crate) const ID52_MODEL_ID: &str = "38670001";

/// `#MapRev` of the files this driver understands.
///
/// The radio can be told to write an **earlier firmware version** of the file
/// (SET > SD Card > Save Form), which is exactly the kind of switch that moves a
/// memory map. Anything that patches measured offsets must check this rather
/// than assume.
pub(crate) const ID52_MAP_REV: u32 = 3;

/// Decoded image length, 256,576 bytes. Measured from the same file.
pub(crate) const ID52_IMAGE_LEN: usize = 0x3EA40;

/// Refuse a settings file this driver's measured offsets do not describe.
///
/// Two ways it can be wrong, and the operator needs to be told which: a file
/// from another Icom, or one written to an **earlier firmware's layout**. The
/// radio can be told to do the latter (`SET > SD Card > Save Form`), and so does
/// third-party programming software — which is exactly the switch that would
/// move every address in [`memory`] and [`settings`].
pub(crate) fn check_is_an_id52_file(icf: &icf::IcfFile) -> Result<(), String> {
    if icf.model_id() != ID52_MODEL_ID {
        return Err(format!(
            "That settings file belongs to a different Icom (model {}, not the ID-52's {}). \
             Use a file saved by this radio.",
            icf.model_id(),
            ID52_MODEL_ID
        ));
    }
    match icf.map_rev() {
        Some(rev) if rev == ID52_MAP_REV => Ok(()),
        other => Err(format!(
            "This settings file is layout revision {}, and this app only writes revision {} — \
             the one the radio saves for itself.\n\n\
             Save a fresh file on the radio with SET > SD Card > Save Setting and use that. \
             Files written by third-party programming software declare an older revision even \
             when their contents match, and the radio's own file is the only one whose layout \
             has been verified.\n\n\
             If the radio itself is writing an older revision, check SET > SD Card > Save Form \
             — it should be set to \"Now Ver\".",
            other.map_or_else(|| "unknown".to_string(), |r| r.to_string()),
            ID52_MAP_REV
        )),
    }
}

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

    fn as_codeplug_exporter(&self) -> Option<&dyn crate::radios::driver::CodeplugExporter> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::driver::DriverCapabilities;

    /// File export — the Memory CH CSV — and nothing else. Every other flag
    /// means a cable session this radio has no protocol for. Locking the exact
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
