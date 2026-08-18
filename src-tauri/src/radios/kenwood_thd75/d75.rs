//! The `.d75` container: the microSD config file the TH-D75 writes and reads.
//!
//! A `.d75` is a **0x100-byte header followed by a flat memory image**, and that
//! is the whole format. No compression, no records, no length fields — the
//! header names who wrote the file and which radio it came off, and everything
//! after it is addressed exactly as CHIRP's clone-mode image is (see
//! [`memory`](super::memory)).
//!
//! ```text
//! 0x00  "TH-D75\xFF\xFF"   the WRITER — the radio itself, or "MCP-D75" for Kenwood's PC software
//! 0x08  "V1.03\xFF\xFF\xFF" that writer's version
//! 0x10  "TH-D75" + FF pad   the radio model
//! 0x20  0x00, then FF
//! 0x80  "K2" + FF pad       unexplained, constant across every sample including the D74's
//! 0x100 body
//! ```
//!
//! Measured from six saves off Tim's card plus kitchen's published D74 example,
//! which reads `TH-D74\xFF\xFFV1.08` in the same fields — so the layout is the
//! family's, not this model's. `scratchpad/thd75/FINDINGS.md` §2 has the table.
//!
//! ## Two producers, two lengths
//!
//! The radio writes a **0x50000** body. Kenwood's MCP-D75 software writes a
//! **0x7A300** one — byte-for-byte the clone image CHIRP downloads over a cable
//! — into a file with the same extension, on the same card, offered by the same
//! Load menu. Both are real `.d75` files and only one of them is the shape this
//! driver has measured, which is why [`D75File::parse`] checks the writer id and
//! the body length separately: the operator needs to be told *which* wrong file
//! they picked, and "save one on the radio" is a different instruction from
//! "that file is truncated".
//!
//! ## Patch, don't generate
//!
//! The body is ~327 KB of the operator's radio: APRS beacons and position
//! comments, D-STAR call sign history, the repeater list, program-scan limits,
//! every MENU setting, and a large static blob from 0x25000 up that nothing here
//! has identified. This module hands out the body to be patched in named places
//! and re-emits the rest untouched — the same bargain the FT5D's `BACKUP.dat`
//! writer and the ID-52's `.icf` patcher make.
//!
//! ## Checksum
//!
//! There is no sign of one: kitchen's D74 notes say "no checksum apparent", and
//! two saves taken 2½ minutes apart are byte-identical, which rules out anything
//! seeded by time. **That is not the same as verified** — the FT5D's undocumented
//! 32-bit checksum cost four failed writes and a factory reset before it fell.
//! Phase 4 step 1 of `scratchpad/thd75/PLAN.md` settles it by writing a
//! byte-identical copy of a radio save under a new name and loading it, which
//! risks nothing. Until then this module preserves every byte it does not
//! deliberately change, so any checksum that does exist stays valid unless a
//! patch lands on it.

// The exporter in `memory` is the only caller; the accessors below are reached
// from there and from tests.
#![allow(dead_code)]

/// The fixed header, and where its fields sit inside it.
pub(crate) const HEADER_LEN: usize = 0x100;
const WRITER_AT: usize = 0x00;
const WRITER_LEN: usize = 8;
const VERSION_AT: usize = 0x08;
const VERSION_LEN: usize = 8;
const MODEL_AT: usize = 0x10;
const MODEL_LEN: usize = 16;

/// Body length of a save **the radio wrote for itself**. Kenwood's MCP-D75
/// writes 0x7A300 instead; see the module docs.
pub(crate) const RADIO_BODY_LEN: usize = 0x50000;

/// Text in the writer field of a radio-written save, and of an MCP-D75 one.
pub(crate) const RADIO_WRITER: &str = "TH-D75";
pub(crate) const MCP_WRITER: &str = "MCP-D75";

/// Text in the model field of any TH-D75 file, whoever wrote it.
pub(crate) const MODEL: &str = "TH-D75";

/// A parsed `.d75`: the header verbatim, and the image to patch.
#[derive(Debug, Clone)]
pub(crate) struct D75File {
    header: Vec<u8>,
    body: Vec<u8>,
}

impl D75File {
    /// Parse a `.d75`, refusing anything whose layout this driver has not
    /// measured.
    ///
    /// Deliberately strict about the body length. A file that is the right
    /// *shape* but the wrong *size* is a file whose addresses above the memory
    /// pool are unknown, and the driver would happily patch memories into it and
    /// hand a radio something it has never seen — see
    /// [[no-inferred-addresses-on-hardware]].
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() <= HEADER_LEN {
            return Err(format!(
                "That file is only {} bytes — too short to be a TH-D75 config file, which is a \
                 {HEADER_LEN}-byte header followed by the radio's memory.",
                bytes.len()
            ));
        }
        let model = field(bytes, MODEL_AT, MODEL_LEN);
        if model != MODEL {
            return Err(format!(
                "That config file is not from a TH-D75 (it says {:?}). Use a file this radio \
                 saved for itself.",
                if model.is_empty() { "<unreadable>" } else { &model }
            ));
        }

        let writer = field(bytes, WRITER_AT, WRITER_LEN);
        let body_len = bytes.len() - HEADER_LEN;
        if writer == MCP_WRITER {
            return Err(format!(
                "That file was written by Kenwood's MCP-D75 software, not by the radio. It holds \
                 the full {:#X}-byte clone image, and this app has only measured the {RADIO_BODY_LEN:#X}-byte \
                 file the radio writes for itself.\n\n\
                 On the radio, save one with Menu > Configuration > SD Card > Save Setting, and \
                 pick that.",
                body_len
            ));
        }
        if writer != RADIO_WRITER {
            return Err(format!(
                "That config file was written by {writer:?}, which this app does not know. Use a \
                 file the TH-D75 saved for itself."
            ));
        }
        if body_len != RADIO_BODY_LEN {
            return Err(format!(
                "That config file is {body_len:#X} bytes of data where the TH-D75 writes \
                 {RADIO_BODY_LEN:#X} — it is most likely an interrupted or partial save.\n\n\
                 Save a fresh one on the radio and use that."
            ));
        }

        Ok(Self {
            header: bytes[..HEADER_LEN].to_vec(),
            body: bytes[HEADER_LEN..].to_vec(),
        })
    }

    /// The firmware version the radio stamped into the header, e.g. `"V1.03"`.
    /// Not used as a guard: the body length is the thing that actually decides
    /// whether the measured addresses apply, and one file in the sample declares
    /// this same version at a different length.
    pub(crate) fn writer_version(&self) -> String {
        field(&self.header, VERSION_AT, VERSION_LEN)
    }

    pub(crate) fn body(&self) -> &[u8] {
        &self.body
    }

    pub(crate) fn body_mut(&mut self) -> &mut [u8] {
        &mut self.body
    }

    /// Re-emit the file. Byte-identical to the input unless the body was
    /// patched — the header is carried through verbatim rather than rebuilt,
    /// so nothing in it can drift.
    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.header.len() + self.body.len());
        out.extend_from_slice(&self.header);
        out.extend_from_slice(&self.body);
        out
    }
}

/// Read a `0xFF`-padded ASCII header field. Kenwood pads with `0xFF`, not NUL,
/// and stops at the first pad byte.
fn field(bytes: &[u8], at: usize, len: usize) -> String {
    bytes
        .get(at..at + len)
        .unwrap_or_default()
        .iter()
        .take_while(|&&b| (0x20..0x7F).contains(&b))
        .map(|&b| b as char)
        .collect()
}

// `pub(crate)`, not private: [`synth`] is the fixture the memory encoder's own
// tests build their card files from, so the shape of a `.d75` is described in
// exactly one place.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal radio-shaped file: the header fields this module reads, and a
    /// body of the right length filled with a marker so a patch is visible.
    pub(crate) fn synth(writer: &str, model: &str, body_len: usize) -> Vec<u8> {
        let mut out = vec![0xFF; HEADER_LEN + body_len];
        out[MODEL_AT..MODEL_AT + model.len()].copy_from_slice(model.as_bytes());
        out[WRITER_AT..WRITER_AT + writer.len()].copy_from_slice(writer.as_bytes());
        out[VERSION_AT..VERSION_AT + 5].copy_from_slice(b"V1.03");
        out[0x20] = 0x00;
        out[0x80..0x82].copy_from_slice(b"K2");
        out
    }

    /// The property the whole patch-don't-generate approach rests on: a file
    /// that goes in unchanged comes out identical, header included.
    #[test]
    fn round_trips_byte_identically() {
        let raw = synth(RADIO_WRITER, MODEL, RADIO_BODY_LEN);
        let f = D75File::parse(&raw).expect("parse");
        assert_eq!(f.to_bytes(), raw);
        assert_eq!(f.writer_version(), "V1.03");
        assert_eq!(f.body().len(), RADIO_BODY_LEN);
    }

    /// Each way of being the wrong file gets its own message, because each one
    /// has a different fix: a D74 file is the wrong radio, an MCP-D75 file is
    /// the wrong producer, and a short file is a bad save.
    #[test]
    fn refuses_each_wrong_file_by_name() {
        let other_radio = synth("TH-D74", "TH-D74", RADIO_BODY_LEN);
        let err = D75File::parse(&other_radio).unwrap_err();
        assert!(err.contains("TH-D74"), "{err}");

        let mcp = synth(MCP_WRITER, MODEL, 0x7A300);
        let err = D75File::parse(&mcp).unwrap_err();
        assert!(err.contains("MCP-D75") && err.contains("Save Setting"), "{err}");

        let truncated = synth(RADIO_WRITER, MODEL, RADIO_BODY_LEN - 0x2C00);
        let err = D75File::parse(&truncated).unwrap_err();
        assert!(err.contains("partial save"), "{err}");

        let err = D75File::parse(&[0u8; 16]).unwrap_err();
        assert!(err.contains("too short"), "{err}");
    }

    /// Patching goes through the body, and the header must not move with it —
    /// the body's addresses are what every measured offset is relative to.
    #[test]
    fn patches_land_in_the_body_not_the_header() {
        let raw = synth(RADIO_WRITER, MODEL, RADIO_BODY_LEN);
        let mut f = D75File::parse(&raw).expect("parse");
        f.body_mut()[0] = 0x42;
        let out = f.to_bytes();
        assert_eq!(&out[..HEADER_LEN], &raw[..HEADER_LEN]);
        assert_eq!(out[HEADER_LEN], 0x42);
    }
}
