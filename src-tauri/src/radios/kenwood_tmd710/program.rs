//! Programming a codeplug into a TM-D710, one `ME` line at a time (#113).
//!
//! ## ⚠ This is the only non-atomic write in the app
//!
//! Every other radio here commits a whole image, or patches a file the radio
//! reads at its leisure. This one sends a memory, waits for the radio to take
//! it, and sends the next. Seventeen milliseconds each, a thousand of them.
//!
//! So a failure halfway leaves the radio **half-programmed** — some slots new,
//! some still the operator's — and no other driver in this repo can do that.
//! The consequence is not a caveat in a doc comment, it is a requirement on the
//! error path: when a write fails, the error says **which slot it stopped at and
//! how many landed**, because "programming failed" on a radio in that state is
//! not enough for anyone to act on. The backup transcript is what puts it back.
//!
//! ## The backup is a transcript
//!
//! There is no image to save. The pre-write backup is the radio's own
//! `ME`/`MN` lines for all 1000 slots, in the format `d710_restore` already
//! reads, so a bad program is undone by the harness that has been putting Tim's
//! radio back all campaign. It is taken **before the first byte goes out**, and
//! a failure to take it aborts the program rather than proceeding unprotected.
//!
//! ## What it does not write
//!
//! No zones, no scan lists, no contacts. The radio has ten memory groups and
//! program-scan limit pairs, and neither has been measured — the seed row says
//! `banks_supported: false` for that reason, so a codeplug's channel lists flow
//! into one flat pool of 1000 memories. See `channels-are-radio-agnostic`: this
//! is the "neither zones nor banks" flattening the resolver already does.

use std::path::Path;

use super::encode::{encode_channel, encode_name};
use super::memory::{Memory, MemoryName, EMPTY_REPLY, MAX_NAME};
use super::{ask, open_port, write_memory, write_name};
use crate::commands::export::{exclusion_reason, expanded_names};
use crate::radios::driver::{
    CodeplugPayload, CodeplugPreview, CodeplugProgrammer, ProgramReport, SkippedChannel,
};

/// What a program run will do, resolved without touching the port.
#[derive(Debug)]
pub(crate) struct Plan {
    radio: String,
    /// Memory and name per slot, packed contiguously from slot 0.
    memories: Vec<(Memory, MemoryName)>,
    skipped: Vec<SkippedChannel>,
    warnings: Vec<String>,
}

impl Plan {
    fn preview(&self) -> CodeplugPreview {
        CodeplugPreview {
            radio: self.radio.clone(),
            channels: self.memories.len(),
            zones: 0,
            scan_lists: 0,
            contacts: 0,
            zone_names: Vec::new(),
            scan_list_names: Vec::new(),
            skipped: self.skipped.clone(),
            warnings: self.warnings.clone(),
        }
    }
}

/// Resolve the payload into lines, with no hardware and no side effects.
///
/// Per-channel problems become `skipped` entries carrying the reason, never
/// errors: a codeplug with one 159.8 Hz tone in it should still program the
/// other sixty-one channels, and the operator should be told which one did not
/// go. Only structural problems — the wrong model, more channels than the radio
/// has slots — stop the run.
pub(crate) fn plan(payload: &CodeplugPayload) -> Result<Plan, String> {
    let model = payload.model;
    if model.model != "TM-D710" {
        return Err(format!(
            "live-mode programming is only wired up for the TM-D710 (codeplug targets {})",
            model.display_name
        ));
    }
    let max_slots = model.memory_channels.unwrap_or(1000) as usize;

    // ⚠ Computed for the WHOLE payload up front, because disambiguation is a
    // property of the set: `expanded_names` can only tell two `W0QEY` repeaters
    // apart by seeing both. Every other radio here does this; the TM-D710 called
    // the singular `expanded_name` per channel and so shipped duplicate names to
    // the radio while the export preview showed them pulled apart (#26).
    //
    // Indexed by position in `payload.channels`, including the excluded ones, so
    // a skipped channel cannot shift the names of the ones after it.
    let names = expanded_names(payload.channels.iter(), model);

    let mut memories = Vec::new();
    let mut skipped = Vec::new();
    // (what the app calls the channel, what the radio will keep) — so the
    // truncation warning can compare them instead of guessing from a length.
    let mut planned_names: Vec<(String, String)> = Vec::new();
    for (ec, name) in payload.channels.iter().zip(names) {
        // Band and mode fit first — the same verdict the export preview shows,
        // so the two cannot disagree about which channels are in.
        if let Some(reason) = exclusion_reason(&ec.channel, model) {
            skipped.push(SkippedChannel { name, reason });
            continue;
        }
        let slot = memories.len();
        if slot >= max_slots {
            return Err(format!(
                "codeplug expands to more than the {max_slots} memories a TM-D710 has — trim the \
                 channel lists"
            ));
        }
        // Then whether this radio can express the channel at all. The encoder
        // refuses rather than substituting a near value, and its message names
        // the reason — a tone the radio does not have, an offset past 29.95 MHz.
        match encode_channel(slot as u16, &ec.channel) {
            Ok(m) => {
                let mut n = encode_name(slot as u16, &ec.channel);
                // ⚠ `name` comes from `expanded_names` (PLURAL), which appends
                // talkgroup labels AND pulls collisions apart. The singular
                // `expanded_name` does NOT disambiguate — a comment here used to
                // claim it did — so two W0QEY repeaters both reached the radio as
                // `W0QEY Fo` while every export path showed `W0QEY V`/`W0QEY U`.
                //
                // ⚠ And an empty result is dropped rather than written: a channel
                // with no long or short name yields `""` here, which would
                // overwrite `encode_name`'s call-sign fallback and send `MN nnn,`.
                if !name.is_empty() {
                    n.text = super::encode::sanitize_name(&name);
                    planned_names.push((name.clone(), n.text.clone()));
                }
                memories.push((m, n));
            }
            Err(reason) => skipped.push(SkippedChannel { name, reason }),
        }
    }

    let mut warnings = Vec::new();
    if memories.len() > 1 {
        warnings.push(format!(
            "The TM-D710 is programmed one memory at a time, so this write is not atomic: {} \
             memories go out individually and a failure partway leaves the radio holding some of \
             each. A full transcript of the radio is saved first and can be restored.",
            memories.len()
        ));
    }
    if name_is_truncated(&planned_names) {
        warnings.push(format!(
            "Some channel names are longer than the {MAX_NAME} characters this radio keeps and \
             have been shortened."
        ));
    }

    Ok(Plan {
        radio: model.display_name.clone(),
        memories,
        skipped,
        warnings,
    })
}

/// Whether any name actually lost characters.
///
/// ⚠ Compares the name against the source it came from. Testing
/// `len() == MAX_NAME` warned about every name that merely *filled* the field —
/// a codeplug whose longest name was exactly `SIMPLEX8` told the operator names
/// "have been shortened" when nothing had been.
fn name_is_truncated(planned: &[(String, String)]) -> bool {
    planned.iter().any(|(source, kept)| source != kept)
}

impl CodeplugProgrammer for super::KenwoodTmD710 {
    fn preview(&self, payload: &CodeplugPayload) -> Result<CodeplugPreview, String> {
        Ok(plan(payload)?.preview())
    }

    fn program(
        &self,
        port: &str,
        payload: &CodeplugPayload,
        backup_dir: &Path,
    ) -> Result<ProgramReport, String> {
        let plan = plan(payload)?;
        let mut p = open_port(port)?;

        // Identity first. Every command below is a write, and sending `ME` lines
        // at a radio that turns out to be a TM-V71 would program the wrong set.
        //
        // ⚠ Through `confirm_model`, which is the SAME check `identify` applies.
        // This used to be `id.contains("TM-D710")` — and `"TM-D710G"` contains
        // `"TM-D710"`, so the one radio `identify` refuses by name could still be
        // sent 1000 `ME` lines plus a clear pass from here.
        super::confirm_model(&mut *p)?;

        // ── The backup, before anything goes out ───────────────────────────
        std::fs::create_dir_all(backup_dir).map_err(|e| e.to_string())?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let backup_path = backup_dir.join(format!("kenwood_tmd710-{stamp}.txt"));
        let mut transcript = String::new();
        let mut occupied: Vec<u16> = Vec::new();
        for slot in 0..1000u16 {
            let line = ask(&mut *p, &format!("ME {slot:03}"))?;
            if line == EMPTY_REPLY {
                continue;
            }
            occupied.push(slot);
            transcript.push_str(&line);
            transcript.push('\n');
            transcript.push_str(&ask(&mut *p, &format!("MN {slot:03}"))?);
            transcript.push('\n');
        }
        std::fs::write(&backup_path, &transcript).map_err(|e| {
            format!("could not save the pre-write backup, so nothing was written: {e}")
        })?;

        // ── The write ──────────────────────────────────────────────────────
        let mut channels_written = 0usize;
        for (m, n) in &plan.memories {
            // Both of these read the slot back and compare the whole line, which
            // on a protocol with no checksum is the only evidence there is.
            write_memory(&mut *p, m).map_err(|e| stopped_at(m.slot, channels_written, &backup_path, &e))?;
            write_name(&mut *p, n).map_err(|e| stopped_at(m.slot, channels_written, &backup_path, &e))?;
            channels_written += 1;
        }

        // Slots the radio held that this codeplug does not fill. Cleared so a
        // program is a full replace and not a merge with whatever was there.
        let mut slots_cleared = 0usize;
        for slot in occupied.iter().copied().filter(|s| (*s as usize) >= plan.memories.len()) {
            // ⚠ Read back, like every write above. A non-`?` reply is not evidence
            // on this protocol: a well-formed line the radio chooses to ignore
            // draws nothing at all, and this counted such a slot as cleared — so a
            // previous codeplug's channel stayed live while the report said it was
            // gone. `d710_clear_slots` verifies the same way.
            ask(&mut *p, &format!("ME {slot:03},C"))
                .map_err(|e| clear_failed(slot, slots_cleared, &backup_path, &e))?;
            let after = ask(&mut *p, &format!("ME {slot:03}"))
                .map_err(|e| clear_failed(slot, slots_cleared, &backup_path, &e))?;
            if after != EMPTY_REPLY {
                return Err(clear_failed(
                    slot,
                    slots_cleared,
                    &backup_path,
                    &format!("the radio still reports {after}"),
                ));
            }
            slots_cleared += 1;
        }

        Ok(ProgramReport {
            channels_written,
            slots_cleared,
            zones_written: 0,
            zones_cleared: 0,
            scan_lists_written: 0,
            scan_lists_cleared: 0,
            contacts_written: 0,
            contacts_cleared: 0,
            // No flash on a live-mode radio — there are no windows to name.
            windows_written: Vec::new(),
            backup_path: backup_path.to_string_lossy().into_owned(),
            // Nothing to byte-verify against: the verification already happened,
            // per memory, as the read-back inside every write.
            expected_path: String::new(),
            warnings: plan.warnings.clone(),
            note: format!(
                "Every memory was read back and matched, and every cleared slot was read back \
                 empty. {channels_written} memories written, {slots_cleared} cleared."
            ),
        })
    }
}

/// The error a half-programmed radio needs.
///
/// ⚠ This is the message that makes the non-atomic write survivable. "Writing
/// failed" tells an operator nothing when the radio now holds a mixture; the
/// slot it stopped at, the count that landed, and the path back are the three
/// things they need.
fn stopped_at(slot: u16, written: usize, backup: &Path, cause: &str) -> String {
    format!(
        "Programming stopped at memory {slot:03}. {written} memories were written before it, and \
         the radio is now holding a MIXTURE of the new codeplug and what it had. The radio's \
         original contents were saved to {} first and can be restored.\n\nCause: {cause}",
        backup.display()
    )
}

/// The error the CLEAR pass needs, which is a different situation.
///
/// ⚠ This used to reuse [`stopped_at`], which told the operator "Programming
/// stopped at memory 500, 62 memories were written before it, the radio is
/// holding a MIXTURE" — when in fact every memory had been written correctly and
/// only a leftover slot from a previous codeplug failed to clear. The right
/// action is different too: nothing needs rewriting, one stale slot needs
/// removing.
fn clear_failed(slot: u16, cleared: usize, backup: &Path, cause: &str) -> String {
    format!(
        "All memories were written and verified, but clearing leftover memory {slot:03} failed \
         after {cleared} slot(s) had been cleared. The new codeplug IS on the radio; what remains \
         is one or more channels from what was there before, at memory {slot:03} and possibly \
         above it. The radio's original contents were saved to {} first.\n\nCause: {cause}",
        backup.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::export::ExpandedChannel;
    use crate::models::{Channel, RadioModel};

    fn model() -> RadioModel {
        let mut m: RadioModel = serde_json::from_value(serde_json::json!({
            "id": 1, "manufacturer": "Kenwood", "model": "TM-D710",
            "display_name": "Kenwood TM-D710", "analog_capable": true,
            "dmr_capable": false, "dstar_capable": false, "ysf_capable": false,
            "nxdn_capable": false, "p25_capable": false, "m17_capable": false,
            "aprs_capable": true, "covers_hf": false, "covers_vhf": true,
            "covers_uhf": true, "covers_220": true, "covers_900": false,
            "freq_min": 144.0, "freq_max": 450.0,
            "tx_bands": "[[144.0,148.0],[430.0,450.0]]",
            "rx_bands": "[[118.0,523.995]]", "memory_channels": 1000,
            "zones_supported": false, "scan_lists_supported": false,
            "banks_supported": false, "max_name_length": 8,
            "export_format": "chirp_csv", "connection_type": "Serial cable",
            "non_channel_settings_schema": "[]", "driver_key": "kenwood_tmd710",
            "programming_ui": "generic"
        }))
        .expect("model");
        m.memory_channels = Some(1000);
        m
    }

    fn ec(rx: f64, name: &str, tone: Option<f64>) -> ExpandedChannel {
        ExpandedChannel {
            channel: Channel {
                rx_freq: rx,
                name_short: Some(name.into()),
                mode: Some("FM".into()),
                tone_mode: tone.map(|_| "Tone".to_string()),
                ctcss_uplink: tone,
                ..Default::default()
            },
            tg_label: None,
            timeslot: None,
            tg_number: None,
            tg_call_type: None,
            tg_inline: false,
        }
    }

    fn payload<'a>(model: &'a RadioModel, chans: &'a [ExpandedChannel]) -> CodeplugPayload<'a> {
        CodeplugPayload {
            model,
            groups: &[],
            channels: chans,
            scan_lists: &[],
            scan_list_overrides: &[],
        }
    }

    /// Channels pack from slot 0 in order, with the app's own name.
    #[test]
    fn channels_pack_contiguously_from_slot_zero() {
        let m = model();
        let chans = [ec(146.520, "SIMPLEX", None), ec(446.000, "UHF", Some(100.0))];
        let plan = plan(&payload(&m, &chans)).unwrap();
        assert_eq!(plan.memories.len(), 2);
        assert_eq!(plan.memories[0].0.slot, 0);
        assert_eq!(plan.memories[1].0.slot, 1);
        assert_eq!(plan.memories[0].1.text, "SIMPLEX");
        assert!(plan.skipped.is_empty());
    }

    /// ★ A channel this radio cannot express is SKIPPED with the encoder's own
    /// reason — not substituted, and not fatal to the other sixty-one. 159.8 Hz
    /// is a real tone on other radios in this library and not on this one.
    #[test]
    fn a_channel_the_radio_cannot_express_is_skipped_not_fatal() {
        let m = model();
        let chans = [
            ec(146.520, "GOOD", None),
            ec(146.940, "ODDTONE", Some(159.8)),
            ec(147.000, "ALSOGOOD", None),
        ];
        let plan = plan(&payload(&m, &chans)).unwrap();
        assert_eq!(plan.memories.len(), 2, "the other two still program");
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].name, "ODDTONE");
        assert!(plan.skipped[0].reason.contains("159.8"), "{:?}", plan.skipped[0]);
        // And the survivors close up behind it, so no slot is left empty.
        assert_eq!(plan.memories[1].0.slot, 1);
    }

    /// Out of band is skipped by the same fit rule the export preview uses, so
    /// the two screens cannot disagree about what is in the codeplug.
    #[test]
    fn an_out_of_coverage_channel_is_skipped_by_the_shared_fit_rule() {
        let m = model();
        let chans = [ec(146.520, "IN", None), ec(800.0, "OUT", None)];
        let plan = plan(&payload(&m, &chans)).unwrap();
        assert_eq!(plan.memories.len(), 1);
        assert_eq!(plan.skipped[0].name, "OUT");
    }

    /// A 220 MHz repeater is RECEIVE-ONLY on this radio, not excluded — it must
    /// still get a memory. This is the case that separates the TM-D710 from the
    /// TH-D72 beside it in the seed.
    #[test]
    fn a_220_repeater_still_gets_a_memory() {
        let m = model();
        let chans = [ec(224.840, "220RPT", None)];
        let plan = plan(&payload(&m, &chans)).unwrap();
        assert_eq!(plan.memories.len(), 1, "{:?}", plan.skipped[0]);
        assert_eq!(plan.memories[0].0.rx_hz, 224_840_000);
    }

    /// ★ Hardware ladder step 4, the desk half: **a channel at each edge of the
    /// claimed coverage**, held against the seed's own numbers.
    ///
    /// The edges are not invented here. `d710_rx_band_sweep` measured them on
    /// the radio by refusal — this model validates an `ME` write and rejects it
    /// whole, so acceptance is a measurement — and a 1350-point sweep put the
    /// receiver at **118.000-523.995 MHz**, which is what the seed row says.
    /// This test is what stops the two drifting: if someone edits `rx_bands`,
    /// a frequency the radio was measured to accept starts being dropped, and
    /// an out-of-coverage frequency becomes a **silently empty memory slot**
    /// while the app reports success. That has cost three repeaters before.
    #[test]
    fn every_edge_of_the_measured_coverage_still_gets_a_memory() {
        let m = model();
        // Inside: both TX bands' edges, and the receive-only extremes.
        for mhz in [118.0, 144.0, 148.0, 224.84, 430.0, 450.0, 523.995] {
            let chans = [ec(mhz, "EDGE", None)];
            let plan = plan(&payload(&m, &chans)).unwrap();
            assert_eq!(
                plan.memories.len(),
                1,
                "{mhz} MHz is inside the measured coverage and was skipped: {:?}",
                plan.skipped
            );
        }
        // Outside, on both sides. A dropped channel must be REPORTED, which is
        // the difference between a skipped row and an empty slot nobody notices.
        for mhz in [117.995, 524.0] {
            let chans = [ec(mhz, "PAST", None)];
            let plan = plan(&payload(&m, &chans)).unwrap();
            assert!(plan.memories.is_empty(), "{mhz} MHz is outside the measured coverage");
            assert_eq!(plan.skipped.len(), 1, "{mhz} MHz was dropped without a reason");
            assert_eq!(plan.skipped[0].name, "PAST");
        }
    }

    /// The non-atomic warning is not decoration: it is the one thing about this
    /// radio an operator cannot infer from any other radio's behaviour.
    #[test]
    fn the_preview_warns_that_the_write_is_not_atomic() {
        let m = model();
        let chans = [ec(146.520, "A", None), ec(147.000, "B", None)];
        let preview = plan(&payload(&m, &chans)).unwrap().preview();
        assert!(
            preview.warnings.iter().any(|w| w.contains("not atomic")),
            "{:?}",
            preview.warnings
        );
        assert_eq!(preview.zones, 0, "the radio's grouping has not been measured");
    }

    /// Over capacity is structural and stops the run, rather than silently
    /// dropping the tail.
    #[test]
    fn more_channels_than_the_radio_holds_is_an_error() {
        let mut m = model();
        m.memory_channels = Some(2);
        let chans = [
            ec(146.520, "A", None),
            ec(147.000, "B", None),
            ec(147.100, "C", None),
        ];
        let err = plan(&payload(&m, &chans)).unwrap_err();
        assert!(err.contains("more than the 2 memories"), "{err}");
    }

    /// Programming a codeplug aimed at another radio must not reach the port.
    #[test]
    fn a_codeplug_for_another_radio_is_refused() {
        let mut m = model();
        m.model = "TM-V71".into();
        m.display_name = "Kenwood TM-V71".into();
        let err = plan(&payload(&m, &[])).unwrap_err();
        assert!(err.contains("only wired up for the TM-D710"), "{err}");
    }

    /// ★ The message a half-programmed radio needs. Asserted because it is the
    /// only mitigation this modality has, and a generic "write failed" would
    /// leave an operator with no idea what state their radio is in.
    #[test]
    fn the_failure_message_names_the_slot_the_count_and_the_way_back() {
        let msg = stopped_at(42, 41, Path::new("/tmp/backup.txt"), "no reply to \"ME 042\"");
        assert!(msg.contains("042"), "{msg}");
        assert!(msg.contains("41 memories were written"), "{msg}");
        assert!(msg.contains("MIXTURE"), "{msg}");
        assert!(msg.contains("/tmp/backup.txt"), "{msg}");
    }
}
