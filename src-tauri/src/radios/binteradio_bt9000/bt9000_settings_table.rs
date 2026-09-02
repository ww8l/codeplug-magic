//! BT-9000 settings field table — GENERATED, do not edit.
//!
//! Source: `scratchpad/binteradio_bt9000/MEASURED.md`, via
//! `gen_bt9000_settings.py`. The profile-form schema at
//! `src/bt9000_settings_schema.json` comes from the same parse, and
//! `settings.rs` asserts the two still describe the same fields.
//!
//! Every field here is graded `screen` in the sheet: its encoding was
//! settled on the radio's own screen, not taken from the source
//! inventory or the manual. That bar is higher here than on other
//! radios in this crate because THIS RADIO VALIDATES NOTHING — it
//! stored 127 in four fields whose maxima are 9, 2, 3 and 1 — so a
//! wrong encoding is stored rather than refused, and this table is the
//! only thing standing between the operator and a bad value.
//!
//! 3 field(s). The sheet's other rows are measured but not
//! settled; see its Tally section for what is still owed.

/// How a field's value is carried in the byte.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Enc {
    /// Stored byte is the value: an enum index, or the displayed number.
    Direct,
    /// Stored byte is the displayed number minus one. Real on this radio:
    /// SQL at 0x00 stores the level, VOX Level at 0x02 stores level − 1,
    /// and they sit two bytes apart with the same printed "Level 1-9".
    Minus1,
}

/// What the form draws, and what the encoder may write.
///
/// ⚠ No settled field is currently a bool, so that
/// variant is unconstructed today. It is kept because the sheet has rows
/// of that kind waiting on a screen check, and deleting it would mean
/// rewriting the encoder when they land.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Kind {
    /// `0 = OFF`, `1 = ON`.
    Bool,
    /// Zero-based index into `options`, which is in STORED order.
    Enum,
    /// Displayed number, inclusive of both bounds.
    Int { lo: u8, hi: u8 },
}

/// One settings field.
pub(crate) struct SF {
    pub key: &'static str,
    pub label: &'static str,
    /// This radio's own menu path. NOT the RT-950 Pro manual's: every
    /// item in the Radio group is numbered one higher here, because the
    /// BT-9000 inserts Work Band at Radio → 1.
    pub menu: &'static str,
    /// Offset within the function block (file offset = 0x7900 + addr).
    pub addr: usize,
    pub kind: Kind,
    pub enc: Enc,
    /// Enum labels in stored-index order; empty for the other kinds.
    pub options: &'static [&'static str],
}

pub(crate) const FIELDS: [SF; 3] = [
    SF {
        key: "vox-level",
        label: "VOX Level",
        menu: "VOX → 2. VOX Level",
        addr: 0x02,
        kind: Kind::Int { lo: 1, hi: 9 },
        enc: Enc::Minus1,
        options: &[],
    },
    SF {
        key: "squelch",
        label: "SQL",
        menu: "Radio → 2. SQL",
        addr: 0x00,
        kind: Kind::Int { lo: 1, hi: 9 },
        enc: Enc::Direct,
        options: &[],
    },
    SF {
        key: "power-on-display",
        label: "Power On Display",
        menu: "Setting → 6. Power On Display",
        addr: 0x1C,
        kind: Kind::Enum,
        enc: Enc::Direct,
        options: &["Picture", "Voltage"],
    },
];
