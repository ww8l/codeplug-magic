//! TYT MD-380 driver (`driver_key = "tyt_md380"`, issue #42).
//!
//! A DMR + analog UHF handheld with no serial port: switched on normally with
//! the cable in, the radio enumerates as an STM32 DFU device (0483:DF11) and
//! its 256 KiB codeplug is read and written whole over USB control transfers.
//! The layout is shared by the MD-380G, MD-390 and Retevis RT3/RT8.
//!
//! - `layout`   — table offsets and record sizes.
//! - `memory`   — channel / zone / contact / RX group / scan list records.
//! - `program`  — the codeplug build: a database codeplug into a read image.
//! - `settings` — General Settings, side buttons and menu switches.
//! - `stt`      — Windows: the same requests through ST's STTub30 driver.
//! - `protocol` — the DFU session. Read measured on Tim's radio; write is
//!   dmrconfig's sequence against a fake, NOT yet run on hardware.
//!
//! The `port` every trait method takes is ignored: the radio is found on the
//! USB bus by its id and confirmed by its ident. The dialog passes a fixed
//! token, which also keys the one-operation-at-a-time port lock.

pub(crate) mod layout;
pub(crate) mod memory;
pub(crate) mod program;
pub(crate) mod protocol;
pub(crate) mod settings;
#[cfg(windows)]
mod stt;
#[cfg(test)]
mod real_images;
#[cfg(test)]
mod dev_export;
#[cfg(test)]
mod hw_ladder;

use std::path::Path;

use crate::radios::driver::{
    with_restore_hint, CodeplugPayload, CodeplugPreview, CodeplugProgrammer,
    DecodedChannelSample, ImageReader, ImageRestorer, ProgramReport, RadioDriver,
    RadioIdentity,
};

use layout::{at, CHANNELS, CHANNEL_COUNT, CHANNEL_LEN, IMAGE_LEN};
use memory::{Channel, Mode, Tone};
use protocol::{Ident, Link};

pub(crate) struct TytMd380;

/// Registry entry (see `radios/registry.rs`).
pub(crate) static DRIVER: TytMd380 = TytMd380;

/// The band this driver's model row describes. The radio reports its own; a
/// unit from another band is refused for writes, because the codeplug was
/// filtered against 400-480 and would land as silently empty memories.
const BAND: (f64, f64) = (400.0, 480.0);

impl RadioDriver for TytMd380 {
    fn key(&self) -> &'static str {
        "tyt_md380"
    }

    fn display_name(&self) -> &'static str {
        "TYT MD-380"
    }

    /// Not a serial radio; there is no baud rate.
    fn baud(&self) -> u32 {
        0
    }

    /// Descriptor only: the radio's own ident needs programming mode, which
    /// ends in a restart, and Identify must leave the radio as it was. The
    /// model and band are checked on every read and write.
    fn identify(&self, _port: &str) -> Result<RadioIdentity, String> {
        let found = protocol::probe()?;
        Ok(RadioIdentity { matched: "MD-380".into(), ident_hex: String::new(), ident_ascii: Some(found) })
    }

    fn usb_direct(&self) -> bool {
        true
    }

    /// It backs up an image but is programmed from the codeplug
    /// (`CodeplugProgrammer`), not as an image.
    fn as_image_reader(&self) -> Option<&dyn ImageReader> {
        Some(self)
    }

    fn as_image_restorer(&self) -> Option<&dyn ImageRestorer> {
        Some(self)
    }

    fn as_codeplug_programmer(&self) -> Option<&dyn CodeplugProgrammer> {
        Some(self)
    }

    fn as_settings_reader(&self) -> Option<&dyn crate::radios::driver::SettingsReader> {
        Some(self)
    }

    fn as_settings_writer(&self) -> Option<&dyn crate::radios::driver::SettingsWriter> {
        Some(self)
    }
}

fn identity(id: &Ident) -> RadioIdentity {
    RadioIdentity {
        matched: id.model.clone(),
        ident_hex: protocol::hex(&id.raw),
        ident_ascii: Some(format!("{} {:.0}-{:.0} MHz", id.model, id.low_mhz, id.high_mhz)),
    }
}

/// One programming-mode session: open, identify, run `f`, and always leave
/// programming mode — which restarts the radio. It re-enumerates on its own
/// (measured s136, Mac and Windows), so the next session needs no hands.
pub(super) fn session<T>(f: impl FnOnce(&mut dyn Link, &Ident) -> Result<T, String>) -> Result<T, String> {
    let mut link = protocol::open()?;
    let link = &mut *link;
    let result = protocol::begin(link).and_then(|id| f(link, &id));
    protocol::reboot(link);
    result
}

pub(super) fn check_band(id: &Ident) -> Result<(), String> {
    if (id.low_mhz, id.high_mhz) != BAND {
        return Err(format!(
            "This MD-380 is a {:.0}-{:.0} MHz unit; the app's MD-380 is the {:.0}-{:.0} MHz \
             model, so a codeplug built for it would not fit this radio. Nothing was written.",
            id.low_mhz, id.high_mhz, BAND.0, BAND.1
        ));
    }
    Ok(())
}

/// The programmed channels, for the download sample and the program report.
fn decode_channels(image: &[u8]) -> Vec<DecodedChannelSample> {
    let tone = |t: Tone| match t {
        Tone::Ctcss(d) => format!("T {:.1}", d as f64 / 10.0),
        Tone::Dcs { code, inverted } => format!("D{code:03}{}", if inverted { "I" } else { "N" }),
        Tone::None(_) => String::new(),
    };
    (0..CHANNEL_COUNT)
        .filter_map(|i| {
            let raw = &image[at(CHANNELS, CHANNEL_LEN, i)..][..CHANNEL_LEN];
            let c = Channel::decode(raw).filter(|_| Channel::is_used(raw))?;
            let rx = memory::ten_hz_to_mhz(c.rx_10hz);
            let tx = memory::ten_hz_to_mhz(c.tx_10hz);
            let shift = if c.rx_only {
                "RX-only".to_string()
            } else if c.tx_10hz == c.rx_10hz {
                String::new()
            } else {
                format!("{:+.3}", tx - rx)
            };
            let tone = match c.mode {
                Mode::Digital => format!("CC{} TS{}", c.color_code, c.slot),
                Mode::Analog => match (tone(c.tx_tone), tone(c.rx_tone)) {
                    (t, r) if t.is_empty() && r.is_empty() => "—".into(),
                    (t, r) if t == r || r.is_empty() => t,
                    (t, r) => format!("{t} / {r}"),
                },
            };
            Some(DecodedChannelSample {
                index: i + 1,
                name: c.name,
                rx_mhz: rx,
                shift: Some(shift),
                tone,
                power: if c.high_power { "High" } else { "Low" }.into(),
                mode: Some(match c.mode {
                    Mode::Digital => "DMR".into(),
                    Mode::Analog => match c.bandwidth {
                        memory::Bandwidth::Narrow => "NFM".into(),
                        _ => "FM".into(),
                    },
                }),
            })
        })
        .collect()
}

/// Read the radio back after a write, from a NEW session — never the session
/// that wrote, whose read-back could be answered from a buffer.
///
/// ★ Measured s136 (ladder rung 1, four writes): the first read after the
/// restart that follows a WRITE returned one random 1 KiB block as blank flash
/// (blocks 162, 50, 45, 47) — and a plain read seconds later held exactly what
/// was sent. Reads after a read-only restart were clean 7 of 7. So the radio
/// is still busy with its new codeplug when it reappears on USB.
///
/// A read whose only differences are whole blank blocks is therefore retried
/// after a pause. A block truly lost in the write would stay blank on every
/// try, so this cannot turn a failed write into a pass: `verified` is an exact
/// match or nothing. Returns the last read and a line per attempt.
pub(super) fn verify_after_write(expected: &[u8]) -> Result<(bool, Vec<u8>, Vec<String>), String> {
    let mut log = Vec::new();
    let mut last = Vec::new();
    for attempt in 1..=3 {
        protocol::wait_for_radio()?;
        if attempt > 1 {
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
        let back = session(|link, _| protocol::read_image(link))?;
        let bad: Vec<usize> = (0..back.len() / 1024)
            .filter(|&b| back[b * 1024..][..1024] != expected[b * 1024..][..1024])
            .collect();
        let blank = bad.iter().all(|&b| back[b * 1024..][..1024].iter().all(|&x| x == 0xFF));
        log.push(format!("read {attempt}: {} blocks differ {bad:?}", bad.len()));
        last = back;
        if bad.is_empty() {
            return Ok((true, last, log));
        }
        if !blank {
            break;
        }
    }
    Ok((false, last, log))
}

/// What the operator does when a write fails part-way.
pub(super) fn restore_hint(e: String, backup: &Path) -> String {
    with_restore_hint(
        e,
        backup,
        "Switch the radio off and on, then use \"Restore backup\" in the Program dialog \
         with this file.",
    )
}

impl ImageReader for TytMd380 {
    fn download_image(&self, _port: &str) -> Result<(RadioIdentity, Vec<u8>), String> {
        session(|link, id| Ok((identity(id), protocol::read_image(link)?)))
    }

    fn decode_sample(&self, image: &[u8]) -> Vec<DecodedChannelSample> {
        decode_channels(image)
    }
}

impl ImageRestorer for TytMd380 {
    fn check_restore_image(&self, image: &[u8]) -> Result<(), String> {
        if image.len() != IMAGE_LEN {
            return Err(format!(
                "this is not an MD-380 backup: it is {} bytes, an MD-380 image is {IMAGE_LEN}",
                image.len()
            ));
        }
        if image[CHANNELS..CHANNELS + CHANNEL_COUNT * CHANNEL_LEN].iter().all(|&b| b == 0xFF) {
            return Err("this image's channel table is blank flash — not a radio read".into());
        }
        Ok(())
    }

    fn restore_image(&self, _port: &str, image: &[u8]) -> Result<(), String> {
        self.check_restore_image(image)?;
        session(|link, id| {
            check_band(id)?;
            protocol::write_image(link, image)
        })?;
        let (verified, _, log) = verify_after_write(image)?;
        if !verified {
            return Err(format!(
                "the restore was written, but reading it back did not match ({})",
                log.join("; ")
            ));
        }
        Ok(())
    }
}

impl CodeplugProgrammer for TytMd380 {
    fn preview(&self, payload: &CodeplugPayload) -> Result<CodeplugPreview, String> {
        Ok(program::plan_program(payload)?.preview())
    }

    fn program(
        &self,
        _port: &str,
        payload: &CodeplugPayload,
        backup_dir: &Path,
    ) -> Result<ProgramReport, String> {
        let plan = program::plan_program(payload)?;
        if plan.channels.is_empty() {
            return Err("nothing in this codeplug can go to an MD-380 — the radio was not touched".into());
        }
        std::fs::create_dir_all(backup_dir).map_err(|e| e.to_string())?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let backup_path = backup_dir.join(format!("tyt_md380-{stamp}.img"));
        let expected_path = backup_dir.join(format!("tyt_md380-{stamp}.expected.img"));

        // One session: the backup read, then the write. The image is planned
        // against the very bytes the backup holds.
        let (base, image) = session(|link, id| {
            check_band(id)?;
            let base = protocol::read_image(link)?;
            let image = plan.apply(&base)?;
            for (path, bytes) in [(&backup_path, &base), (&expected_path, &image)] {
                std::fs::write(path, bytes)
                    .map_err(|e| format!("could not write {}: {e}", path.display()))?;
            }
            protocol::write_image(link, &image).map_err(|e| restore_hint(e, &backup_path))?;
            Ok((base, image))
        })?;
        let before = program::count_used(&base);
        let (verified, back, _) = verify_after_write(&image).map_err(|e| {
            format!("{e}\n\nThe write finished, but the radio could not be read back to check it.")
        })?;
        Ok(ProgramReport {
            channels_written: plan.channels.len(),
            slots_cleared: before.channels.saturating_sub(plan.channels.len()),
            zones_written: plan.zones.len(),
            zones_cleared: before.zones.saturating_sub(plan.zones.len()),
            scan_lists_written: plan.scan_lists.len(),
            scan_lists_cleared: before.scan_lists.saturating_sub(plan.scan_lists.len()),
            contacts_written: plan.contacts.len(),
            contacts_cleared: before.contacts.saturating_sub(plan.contacts.len()),
            windows_written: vec!["0x00000-0x3FFFF".into()],
            backup_path: backup_path.to_string_lossy().into_owned(),
            expected_path: expected_path.to_string_lossy().into_owned(),
            warnings: plan.warnings.clone(),
            note: if verified {
                "Written, and read back after the radio restarted: it holds exactly the \
                 image that was sent."
                    .into()
            } else {
                "Written, but the read-back differs from what was sent. Keep the backup; \
                 restore it if the radio misbehaves."
                    .into()
            },
            verified: Some(verified),
            channels: decode_channels(&back),
            skipped: plan.skipped.clone(),
        })
    }
}
