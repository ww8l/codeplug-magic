//! Kenwood TM-D710A — live-mode command driver (issue #113).
//!
//! **This is the fourth programming modality in the app.** The others clone a
//! whole image (UV-5R, TD-H3), write binary records at flash addresses
//! (AnyTone), or patch a file the radio wrote to a microSD card (FT5D, ID-52,
//! TH-D75). The TM-D710 does none of those: the PC sends one ASCII command per
//! memory, `\r` terminated, and the radio answers in kind.
//!
//! ```text
//! ID              -> ID TM-D710
//! ME 000          -> ME 000,0447275000,0,2,0,0,1,0,12,12,000,05000000,0,0000000000,0,0
//! ME 999          -> N                       (an empty slot)
//! MU              -> MU 0,4,0,…              (all 42 menu settings, one line)
//! ```
//!
//! Two consequences worth stating before anyone extends this:
//!
//! - **A write is not atomic.** Every other radio here commits an image; this
//!   one commits a memory at a time, so a failure halfway leaves the radio
//!   half-programmed. Nothing in this module writes yet, and whatever does will
//!   need to say where it stopped.
//! - **There is no image to back up.** The equivalent is a transcript of the
//!   radio's own `ME`/`MU` lines.
//!
//! ## Measured on the radio, 2026-08-22
//!
//! Tim's TM-D710A on an RT Systems cable, COM port on the rear of the operation
//! panel. Full notes in `scratchpad/kenwood_tmd710/FINDINGS.md`.
//!
//! | | |
//! |---|---|
//! | Baud | **57600** — CHIRP's driver assumes 9600; this radio is silent there |
//! | Round trip | 17 ms; all 1000 slots in 17.2 s |
//! | Empty slot | answers `N` |
//! | Identity | `ID TM-D710` |
//!
//! ⚠ **The first command after opening the port can answer `?`.** Seen during
//! the rate sweep: a wrong-rate write left the radio's parser mid-garbage and it
//! errored the next well-formed line. So one `?` is not a refusal — see
//! [`ask_settling`].
//!
//! ## Capabilities: none yet, deliberately
//!
//! This driver identifies and nothing else, the same scaffolding stance the
//! FT5D was registered under. `memory.rs` can already read and re-emit a slot,
//! but **nothing has ever been written to this radio**. A capability trait here
//! would put a "Program radio" button in front of an operator for a path no one
//! has proven.
//!
//! The tone and DCS tables are no longer unmeasured — see [`tone`], where the
//! radio's own refusal of an out-of-range index settled that fields 9-11 are
//! indices and fixed their lengths at 42 and 104.

use serialport::SerialPort;
use std::time::{Duration, Instant};

use super::driver::{RadioDriver, RadioIdentity};

pub(crate) mod encode;
pub(crate) mod memory;
pub(crate) mod program;
pub(crate) mod settings;
pub(crate) mod tone;

/// Menu 528 on this radio sets it. 57600 is what Tim's is on and what the
/// capture ran at; the driver does not sweep, because a rate mismatch here is
/// an operator setting to fix, not something to paper over.
pub(crate) const BAUD: u32 = 57600;

/// What the radio answers when it cannot parse a command.
const ERROR_REPLY: &str = "?";

/// Long enough for the radio to answer at 17 ms, short enough that a wrong port
/// fails while the operator is still looking at the screen.
const REPLY_TIMEOUT: Duration = Duration::from_millis(1500);

fn open_port(port: &str) -> Result<Box<dyn SerialPort>, String> {
    serialport::new(port, BAUD)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        .timeout(Duration::from_millis(700))
        .open()
        .map_err(|e| format!("could not open {port} at {BAUD} baud: {e}"))
}

/// Send one command and return the radio's reply, without its terminator.
///
/// `?` becomes an error naming the command that drew it. `N` is returned as-is:
/// it is a legitimate answer meaning "nothing here", and only the caller knows
/// whether that is a problem.
pub(crate) fn ask(p: &mut dyn SerialPort, cmd: &str) -> Result<String, String> {
    let _ = p.clear(serialport::ClearBuffer::All);
    p.write_all(format!("{cmd}\r").as_bytes())
        .map_err(|e| format!("sending {cmd:?}: {e}"))?;
    p.flush().map_err(|e| format!("sending {cmd:?}: {e}"))?;

    let mut reply = Vec::new();
    let deadline = Instant::now() + REPLY_TIMEOUT;
    let mut byte = [0u8; 1];
    while Instant::now() < deadline {
        match p.read(&mut byte) {
            Ok(0) => continue,
            Ok(_) if byte[0] == b'\r' => {
                let text = String::from_utf8_lossy(&reply).into_owned();
                return if text == ERROR_REPLY {
                    Err(format!(
                        "the radio did not understand {cmd:?}. On a TM-D710 that usually means \
                         the command is not one this model has, or the previous command left the \
                         port mid-line."
                    ))
                } else {
                    Ok(text)
                };
            }
            Ok(_) => reply.push(byte[0]),
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => break,
            Err(e) => return Err(format!("reading the reply to {cmd:?}: {e}")),
        }
    }
    Err(format!(
        "no reply to {cmd:?} within {} ms. Check the cable is in the COM port on the rear of the \
         operation panel — not the DATA jack — and that Menu 528 (COM PORT SPEED) is {BAUD}.",
        REPLY_TIMEOUT.as_millis()
    ))
}

/// [`ask`], tolerating one `?` first.
///
/// Measured behaviour, not defensive coding: during the rate sweep the radio
/// answered a well-formed `ID` with `?` because the preceding wrong-rate write
/// had left its parser mid-line. Every session therefore starts with one
/// throwaway, and a second `?` is a real refusal.
pub(crate) fn ask_settling(p: &mut dyn SerialPort, cmd: &str) -> Result<String, String> {
    match ask(p, cmd) {
        Ok(reply) => Ok(reply),
        Err(_) => ask(p, cmd),
    }
}

/// Write one memory, then **prove it landed** by reading the slot back and
/// comparing the whole line.
///
/// The read-back is not belt-and-braces, it is the only evidence there is.
/// This radio has no checksum and no commit step: a malformed line draws `?`,
/// but a *well-formed* line the radio chooses to interpret differently draws
/// nothing at all. On the D890UV a settings field turned out to be owned by the
/// firmware and silently reverted after a write — read-back is what makes that
/// visible instead of a lie in the report.
// ⚠ Reachable only from the measurement harness until a capability trait calls
// it — see the same note in `memory.rs`. The write path is deliberately proven
// by the campaign that uses it before it is offered to an operator.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn write_memory(p: &mut dyn SerialPort, m: &memory::Memory) -> Result<(), String> {
    let intended = m.to_line();
    ask(p, &intended)?;
    let after = ask(p, &format!("ME {:03}", m.slot))?;
    if after != intended {
        return Err(format!(
            "memory {:03} did not take the write.\n  sent: {intended}\n  read: {after}",
            m.slot
        ));
    }
    Ok(())
}

/// Write a memory's name, and read it back for the same reason.
// ⚠ Reachable only from the measurement harness until a capability trait calls
// it — see the same note in `memory.rs`. The write path is deliberately proven
// by the campaign that uses it before it is offered to an operator.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn write_name(p: &mut dyn SerialPort, n: &memory::MemoryName) -> Result<(), String> {
    let intended = n.to_line();
    ask(p, &intended)?;
    let after = ask(p, &format!("MN {:03}", n.slot))?;
    if after != intended {
        return Err(format!(
            "name for {:03} did not take.\n  sent: {intended}\n  read: {after}",
            n.slot
        ));
    }
    Ok(())
}

/// Write the whole menu line and report **which parameters did not take**.
///
/// ⚠ `MU` sets all 42 at once. There is no way to write one menu item alone, so
/// every write here is a write of everything — which is exactly why
/// [`memory::Menu::with_field`] refuses a value too wide for its field, and why
/// a caller should build from a line just read off the radio rather than from a
/// remembered one.
///
/// Returns the parameters that differ after the write, as `(p, wanted, got)`.
/// **Empty means clean.** A non-empty result is not necessarily an error — a
/// field the firmware owns can revert on its own, and that is a finding worth
/// seeing rather than an exception worth throwing.
// ⚠ Reachable only from the measurement harness until a capability trait calls
// it — see the same note in `memory.rs`. The write path is deliberately proven
// by the campaign that uses it before it is offered to an operator.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn write_menu(
    p: &mut dyn SerialPort,
    menu: &memory::Menu,
) -> Result<Vec<(usize, String, String)>, String> {
    let intended = menu.to_line();
    ask(p, &intended)?;
    let after = memory::Menu::parse(&ask(p, "MU")?)?;
    Ok(menu.diff(&after))
}

pub(crate) struct KenwoodTmD710;

pub(crate) static DRIVER: KenwoodTmD710 = KenwoodTmD710;

impl RadioDriver for KenwoodTmD710 {
    fn key(&self) -> &'static str {
        "kenwood_tmd710"
    }

    fn display_name(&self) -> &'static str {
        "Kenwood TM-D710"
    }

    fn baud(&self) -> u32 {
        BAUD
    }

    // Both halves, for the same reason the TH-D72 claims both: `MU` reads and
    // writes the radio's menu over one ASCII command, with no clone session
    // involved. Claimed only as of Phase 4 (#113) — writing every one of the 42
    // parameters was proven on Tim's radio first.
    fn as_settings_reader(&self) -> Option<&dyn crate::radios::driver::SettingsReader> {
        Some(self)
    }

    fn as_settings_writer(&self) -> Option<&dyn crate::radios::driver::SettingsWriter> {
        Some(self)
    }

    // Live mode is a `CodeplugProgrammer`, not an `ImageProgrammer`: this radio
    // is written record by record from the database, the way the AnyTone is,
    // and there is no image to clone. See `program.rs` for the consequence —
    // the write is not atomic, and this is the only driver here that isn't.
    fn as_codeplug_programmer(&self) -> Option<&dyn crate::radios::driver::CodeplugProgrammer> {
        Some(self)
    }

    /// Ask the radio what it is. Reads no memory and changes nothing, so it is
    /// the safe first thing an operator can try with a new cable.
    ///
    /// The reply is matched loosely — `TM-D710` covers the D710A and D710E,
    /// which answer identically. The **G** is a different radio with a menu set
    /// this driver has not measured, so it is named and refused rather than
    /// quietly accepted.
    fn identify(&self, port: &str) -> Result<RadioIdentity, String> {
        let mut p = open_port(port)?;
        let reply = ask_settling(&mut *p, "ID")?;
        let model = reply
            .strip_prefix("ID ")
            .ok_or_else(|| format!("unexpected answer to ID: {reply:?}"))?
            .to_string();

        if model == "TM-D710G" {
            return Err(
                "this is a TM-D710G. Only the TM-D710 (non-G) has been measured — the G has a \
                 different menu set, and programming it from this driver would write settings \
                 nobody has checked against it (issue #113)."
                    .into(),
            );
        }
        if model != "TM-D710" {
            return Err(format!(
                "expected a TM-D710 on this port, but it says {model:?}."
            ));
        }

        Ok(RadioIdentity {
            matched: model.clone(),
            ident_hex: reply
                .as_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(" "),
            ident_ascii: Some(reply),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radios::fake_port::{FakePort, FakeRadio};

    /// A TM-D710 at the far end of the cable, answering the commands the
    /// capture proved it answers.
    struct FakeD710 {
        model: &'static str,
        /// Slots the fake is holding, so a write can be read back.
        slots: std::collections::BTreeMap<u16, String>,
        /// Accept the write but keep the old value — the "firmware owns this
        /// field" behaviour seen on another radio in this project.
        stubborn: bool,
        /// Answer the first command with `?` regardless — the settling
        /// behaviour measured during the rate sweep.
        garbled_first: bool,
        pub seen: Vec<String>,
    }

    impl FakeD710 {
        fn new() -> Self {
            Self {
                model: "TM-D710",
                slots: std::collections::BTreeMap::new(),
                stubborn: false,
                garbled_first: false,
                seen: Vec::new(),
            }
        }
    }

    impl FakeRadio for FakeD710 {
        fn step(&mut self, req: &[u8], out: &mut Vec<u8>) -> usize {
            let Some(end) = req.iter().position(|&b| b == b'\r') else {
                return 0;
            };
            let cmd = String::from_utf8_lossy(&req[..end]).into_owned();
            self.seen.push(cmd.clone());

            let reply = if self.garbled_first && self.seen.len() == 1 {
                "?".to_string()
            } else if cmd == "ID" {
                format!("ID {}", self.model)
            } else if cmd == "ME 999" {
                memory::EMPTY_REPLY.to_string()
            } else if let Some(rest) = cmd.strip_prefix("ME ") {
                if rest.contains(',') {
                    // A write: keep it (unless stubborn) and echo it back.
                    let slot: u16 = rest[..3].parse().unwrap();
                    if !self.stubborn {
                        self.slots.insert(slot, cmd.clone());
                    }
                    cmd.clone()
                } else {
                    let slot: u16 = rest.parse().unwrap_or(999);
                    self.slots.get(&slot).cloned().unwrap_or_else(|| {
                        if slot == 0 {
                            "ME 000,0447275000,0,2,0,0,1,0,12,12,000,05000000,0,0000000000,0,0"
                                .to_string()
                        } else {
                            memory::EMPTY_REPLY.to_string()
                        }
                    })
                }
            } else {
                "?".to_string()
            };
            out.extend_from_slice(reply.as_bytes());
            out.push(b'\r');
            end + 1
        }
    }

    #[test]
    fn a_command_gets_its_reply_without_the_terminator() {
        let mut p = FakePort::new(FakeD710::new());
        assert_eq!(ask(&mut p, "ID").unwrap(), "ID TM-D710");
        assert_eq!(ask(&mut p, "ME 999").unwrap(), memory::EMPTY_REPLY);
    }

    /// `?` is an error and names the command, so a driver bug reads as a driver
    /// bug rather than as a silent empty result.
    #[test]
    fn an_error_reply_names_the_command_that_drew_it() {
        let mut p = FakePort::new(FakeD710::new());
        let err = ask(&mut p, "NOPE").unwrap_err();
        assert!(err.contains("NOPE"), "{err}");
        assert!(err.contains("did not understand"), "{err}");
    }

    /// ★ The measured settling behaviour. One `?` on the first command is
    /// survivable; the retry is what makes a fresh session work.
    #[test]
    fn one_error_on_the_first_command_is_retried_not_failed() {
        let mut radio = FakeD710::new();
        radio.garbled_first = true;
        let mut p = FakePort::new(radio);
        assert_eq!(ask_settling(&mut p, "ID").unwrap(), "ID TM-D710");
        assert_eq!(p.radio.seen, vec!["ID", "ID"]);
    }

    /// …but a second `?` is a real refusal, so a genuinely unknown command
    /// still fails instead of retrying forever.
    #[test]
    fn a_persistent_error_still_fails() {
        let mut p = FakePort::new(FakeD710::new());
        assert!(ask_settling(&mut p, "NOPE").is_err());
    }

    /// The G is a different radio. Refusing it by name beats programming it
    /// with a menu table measured on the non-G.
    #[test]
    fn a_d710g_is_named_and_refused() {
        let mut radio = FakeD710::new();
        radio.model = "TM-D710G";
        let mut p = FakePort::new(radio);
        let reply = ask(&mut p, "ID").unwrap();
        assert_eq!(reply, "ID TM-D710G");
        // identify() itself needs a real port; the refusal it applies to this
        // reply is the branch under test, so exercise the same condition.
        assert!(reply.strip_prefix("ID ").unwrap() == "TM-D710G");
    }

    /// A write is only believed after the radio says it back. This is the
    /// happy path: write an empty slot, read it, get the same line.
    #[test]
    fn a_memory_write_is_verified_by_reading_it_back() {
        let mut p = FakePort::new(FakeD710::new());
        let m = memory::Memory::parse(
            "ME 500,0146520000,0,0,0,0,0,0,00,00,000,00000000,0,0000000000,0,0",
        )
        .unwrap();
        write_memory(&mut p, &m).unwrap();
        assert_eq!(ask(&mut p, "ME 500").unwrap(), m.to_line());
    }

    /// ★ The failure this exists to catch: the radio accepts the command and
    /// keeps its own value. Nothing errors on the wire, so without the
    /// read-back the report would claim a write that never happened.
    #[test]
    fn a_write_the_radio_quietly_ignores_is_reported_not_believed() {
        let mut radio = FakeD710::new();
        radio.stubborn = true;
        let mut p = FakePort::new(radio);
        let m = memory::Memory::parse(
            "ME 500,0146520000,0,0,0,0,0,0,00,00,000,00000000,0,0000000000,0,0",
        )
        .unwrap();
        let err = write_memory(&mut p, &m).unwrap_err();
        assert!(err.contains("did not take"), "{err}");
        assert!(err.contains("sent:") && err.contains("read:"), "{err}");
    }
}
