//! Importing a CSV that is not a RepeaterBook export (issue #115).
//!
//! [`super::import`] knows two CSV shapes exactly, by their header rows. Every
//! other CSV an operator has — a CHIRP export, a club's spreadsheet, another
//! programmer's dump — has the same channels in it under different column
//! names, and used to be unimportable.
//!
//! The flow here is inspect → map → preview → import:
//!
//! * [`inspect_csv`] reads the header row and a sample of the data, says
//!   whether the file is a RepeaterBook export after all, and returns a
//!   *guessed* mapping from our channel fields to the file's columns.
//! * The operator corrects the guess in the dialog and sends back a
//!   [`ColumnMapping`].
//! * [`preview_mapped_csv`] parses the whole file under that mapping into the
//!   same preview rows the RepeaterBook importers produce.
//! * [`import_mapped_csv`] inserts them.
//!
//! **These channels are not RepeaterBook records and are not stored as if they
//! were.** They land with `source = 'csv'`, no `repeaterbook_id` and no `rb_*`
//! snapshots, so the re-import merge in `import.rs` — which exists to let a
//! fresh RepeaterBook export correct stale RepeaterBook data while preserving
//! the operator's edits — never touches them. A second import of the same file
//! skips the rows it already has instead of merging them.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use sqlx::{Acquire, SqlitePool};
use tauri::State;

use super::import::{
    build_preview, csv_headers, derive_tone_mode_rb, finalize, parse_leading_f64, parse_rb_tone,
    recognize_csv, ImportPreview, ParsedChannel, RbTone, SourceColumns,
};
use crate::db::AppState;
use crate::error::MapErrString;
use crate::models::ImportSummary;
use crate::util::truncate;

/// Data rows read for the guess and shown as examples under each column.
const SAMPLE_ROWS: usize = 25;

/// Example values shown per column in the mapping dialog.
const SAMPLE_SHOWN: usize = 3;

// ============================================================
// The field catalogue
// ============================================================

/// What kind of value a target field holds. The dialog uses this to say what a
/// column must look like; the parser uses it for nothing — each field has its
/// own reader below.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    Text,
    /// Megahertz, written as a decimal number.
    Freq,
    /// A CTCSS frequency in Hz.
    Tone,
    /// A 3-digit octal DCS code.
    Dcs,
    Number,
    /// One of a fixed set of words; anything else is normalised or dropped.
    Enum,
}

/// A channel field a CSV column can be mapped onto.
#[derive(Debug, Clone, Serialize)]
pub struct MappableField {
    /// Stable key. This is what a [`ColumnMapping`] is keyed on.
    pub key: &'static str,
    pub label: &'static str,
    /// Heading the dialog groups this field under.
    pub group: &'static str,
    pub kind: FieldKind,
    /// Only `rx_freq` is required — a row with no receive frequency is not a
    /// channel.
    pub required: bool,
    /// Shown under the field in the dialog. Empty when the label says it all.
    pub help: &'static str,
}

/// A header name this field answers to, and the value shape it must have.
///
/// The shape is what tells CHIRP's three tone columns apart. CHIRP writes the
/// tone *mode* in a column called `Tone` and the TX tone frequency in
/// `rToneFreq`; RepeaterBook and most spreadsheets write the tone *frequency*
/// in a column called `Tone`. Same header, two different fields, and the values
/// settle it: `TSQL` is a mode, `100.0` is a frequency.
struct Alias {
    /// Normalised header text — lowercase, letters and digits only.
    header: &'static str,
    shape: Shape,
}

#[derive(PartialEq, Clone, Copy)]
enum Shape {
    Any,
    /// Every non-empty sample value parses as a number.
    Numeric,
    /// At least one non-empty sample value does not parse as a number.
    NonNumeric,
}

const fn a(header: &'static str) -> Alias {
    Alias { header, shape: Shape::Any }
}
const fn num(header: &'static str) -> Alias {
    Alias { header, shape: Shape::Numeric }
}
const fn word(header: &'static str) -> Alias {
    Alias { header, shape: Shape::NonNumeric }
}

/// The full catalogue: field, then every header name it answers to.
///
/// Order matters only for the dialog's layout — the guesser cannot produce a
/// collision because a column is claimed once and the ambiguous headers are
/// separated by [`Shape`], not by priority.
struct Field {
    def: MappableField,
    aliases: &'static [Alias],
}

const FIELDS: &[Field] = &[
    // ---- Identity ----
    Field {
        def: MappableField {
            key: "name_long",
            label: "Name",
            group: "Identity",
            kind: FieldKind::Text,
            required: false,
            help: "Left unmapped, a name is built from the callsign and city.",
        },
        aliases: &[a("name"), a("channelname"), a("channel"), a("label"), a("repeatername")],
    },
    Field {
        def: MappableField {
            key: "name_short",
            label: "Short name",
            group: "Identity",
            kind: FieldKind::Text,
            required: false,
            help: "Trimmed to 7 characters, the width the channel editor allows.",
        },
        aliases: &[a("shortname"), a("alias"), a("abbreviation"), a("abbrev")],
    },
    Field {
        def: MappableField {
            key: "callsign",
            label: "Callsign",
            group: "Identity",
            kind: FieldKind::Text,
            required: false,
            help: "",
        },
        aliases: &[a("call"), a("callsign"), a("repeatercall"), a("station")],
    },
    // ---- Frequency ----
    Field {
        def: MappableField {
            key: "rx_freq",
            label: "RX frequency",
            group: "Frequency",
            kind: FieldKind::Freq,
            required: true,
            help: "Megahertz. A row whose value here is not a number is skipped.",
        },
        aliases: &[
            a("outputfreq"),
            a("outputfrequency"),
            a("frequency"),
            a("freq"),
            a("rxfreq"),
            a("rxfrequency"),
            a("receivefrequency"),
            a("downlinkfreq"),
        ],
    },
    Field {
        def: MappableField {
            key: "tx_freq",
            label: "TX frequency",
            group: "Frequency",
            kind: FieldKind::Freq,
            required: false,
            help: "Megahertz. Unmapped, it is computed from the offset and duplex.",
        },
        aliases: &[
            a("inputfreq"),
            a("inputfrequency"),
            a("txfreq"),
            a("txfrequency"),
            a("transmitfrequency"),
            a("uplinkfreq"),
        ],
    },
    Field {
        def: MappableField {
            key: "offset",
            label: "Offset",
            group: "Frequency",
            kind: FieldKind::Freq,
            required: false,
            help: "Megahertz, e.g. 0.6 or -5. Ignored when TX frequency is mapped.",
        },
        aliases: &[num("offset"), a("offsetfreq"), a("offsetfrequency"), a("repeateroffset")],
    },
    Field {
        def: MappableField {
            key: "duplex",
            label: "Duplex",
            group: "Frequency",
            kind: FieldKind::Enum,
            required: false,
            help: "+, -, s/split or blank. Gives the offset its sign.",
        },
        aliases: &[word("offset"), a("duplex"), a("shift"), a("direction"), a("offsetdirection")],
    },
    // ---- Tone ----
    Field {
        def: MappableField {
            key: "tone_mode",
            label: "Tone mode",
            group: "Tone",
            kind: FieldKind::Enum,
            required: false,
            help: "off / Tone / TSQL / DTCS / Cross. Unmapped, it is derived \
                   from whichever tones are present.",
        },
        aliases: &[word("tone"), a("tonemode"), a("tonetype"), a("squelchmode")],
    },
    Field {
        def: MappableField {
            key: "ctcss_uplink",
            label: "TX CTCSS tone",
            group: "Tone",
            kind: FieldKind::Tone,
            required: false,
            help: "Hz. The tone you transmit to open the repeater.",
        },
        aliases: &[
            num("tone"),
            a("uplinktone"),
            a("rtonefreq"),
            a("txtone"),
            a("txctcss"),
            a("ctcsstx"),
            a("txpl"),
            a("pltx"),
            a("pl"),
            a("encode"),
            a("toneencode"),
        ],
    },
    Field {
        def: MappableField {
            key: "ctcss_downlink",
            label: "RX CTCSS tone",
            group: "Tone",
            kind: FieldKind::Tone,
            required: false,
            help: "Hz. The tone the repeater sends back.",
        },
        aliases: &[
            a("downlinktone"),
            a("ctonefreq"),
            a("rxtone"),
            a("rxctcss"),
            a("ctcssrx"),
            a("rxpl"),
            a("plrx"),
            a("tsq"),
            a("decode"),
            a("tonedecode"),
        ],
    },
    Field {
        def: MappableField {
            key: "cross_mode",
            label: "Cross mode",
            group: "Tone",
            kind: FieldKind::Enum,
            required: false,
            help: "Which half is CTCSS and which is DCS, e.g. Tone->DTCS. Only \
                   read when the tone mode is Cross.",
        },
        aliases: &[a("crossmode")],
    },
    Field {
        def: MappableField {
            key: "dcs_code",
            label: "TX DCS code",
            group: "Tone",
            kind: FieldKind::Dcs,
            required: false,
            help: "3-digit octal, with or without a leading D.",
        },
        aliases: &[a("dcs"), a("dtcs"), a("dcscode"), a("dtcscode"), a("txdcs"), a("txdtcs")],
    },
    Field {
        def: MappableField {
            key: "dcs_rx_code",
            label: "RX DCS code",
            group: "Tone",
            kind: FieldKind::Dcs,
            required: false,
            help: "Only when it differs from the TX code.",
        },
        aliases: &[a("rxdcs"), a("rxdtcs"), a("rxdcscode"), a("rxdtcscode")],
    },
    // ---- Signalling ----
    Field {
        def: MappableField {
            key: "mode",
            label: "Mode",
            group: "Signalling",
            kind: FieldKind::Enum,
            required: false,
            help: "FM, DMR, DSTAR, YSF, NXDN, P25, M17. Unmapped, everything is FM.",
        },
        aliases: &[a("mode"), a("modulation"), a("emission")],
    },
    Field {
        def: MappableField {
            key: "power",
            label: "Power",
            group: "Signalling",
            kind: FieldKind::Enum,
            required: false,
            help: "High / Med / Low. A wattage such as 50W is not a level and \
                   is dropped.",
        },
        aliases: &[a("power"), a("powerlevel"), a("txpower")],
    },
    Field {
        def: MappableField {
            key: "dmr_color_code",
            label: "DMR colour code",
            group: "Signalling",
            kind: FieldKind::Number,
            required: false,
            help: "0-15. Anything outside that is dropped, not clamped.",
        },
        aliases: &[a("colorcode"), a("colourcode"), a("dmrcolorcode"), a("dmrcc"), a("cc")],
    },
    Field {
        def: MappableField {
            key: "dmr_timeslot",
            label: "DMR time slot",
            group: "Signalling",
            kind: FieldKind::Number,
            required: false,
            help: "1 or 2.",
        },
        aliases: &[a("timeslot"), a("dmrtimeslot"), a("slot"), a("ts")],
    },
    Field {
        def: MappableField {
            key: "dmr_talkgroup",
            label: "DMR talkgroup",
            group: "Signalling",
            kind: FieldKind::Number,
            required: false,
            help: "The talkgroup's numeric ID.",
        },
        aliases: &[a("talkgroup"), a("talkgroupid"), a("dmrtalkgroup"), a("tgid"), a("tg")],
    },
    Field {
        def: MappableField {
            key: "p25_nac",
            label: "P25 NAC",
            group: "Signalling",
            kind: FieldKind::Text,
            required: false,
            help: "",
        },
        aliases: &[a("nac"), a("p25nac")],
    },
    // ---- Links ----
    Field {
        def: MappableField {
            key: "allstar_node",
            label: "AllStar node",
            group: "Links",
            kind: FieldKind::Text,
            required: false,
            help: "",
        },
        aliases: &[a("allstar"), a("allstarnode"), a("asl")],
    },
    Field {
        def: MappableField {
            key: "echolink_node",
            label: "EchoLink node",
            group: "Links",
            kind: FieldKind::Text,
            required: false,
            help: "",
        },
        aliases: &[a("echolink"), a("echolinknode")],
    },
    Field {
        def: MappableField {
            key: "irlp_node",
            label: "IRLP node",
            group: "Links",
            kind: FieldKind::Text,
            required: false,
            help: "",
        },
        aliases: &[a("irlp"), a("irlpnode")],
    },
    Field {
        def: MappableField {
            key: "wires_node",
            label: "Wires-X node",
            group: "Links",
            kind: FieldKind::Text,
            required: false,
            help: "",
        },
        aliases: &[a("wiresx"), a("wires"), a("wiresnode"), a("wiresxnode")],
    },
    // ---- Location ----
    Field {
        def: MappableField {
            key: "city",
            label: "City",
            group: "Location",
            kind: FieldKind::Text,
            required: false,
            help: "",
        },
        // CHIRP's `Location` column is the memory slot number, not a place, so
        // `location` is only a city when it does not read as a number.
        aliases: &[a("city"), a("town"), word("location")],
    },
    Field {
        def: MappableField {
            key: "county",
            label: "County",
            group: "Location",
            kind: FieldKind::Text,
            required: false,
            help: "",
        },
        aliases: &[a("county")],
    },
    Field {
        def: MappableField {
            key: "state",
            label: "State",
            group: "Location",
            kind: FieldKind::Text,
            required: false,
            help: "A spelled-out name is stored as its postal code.",
        },
        aliases: &[a("state"), a("province"), a("st")],
    },
    Field {
        def: MappableField {
            key: "country",
            label: "Country",
            group: "Location",
            kind: FieldKind::Text,
            required: false,
            help: "",
        },
        aliases: &[a("country"), a("nation")],
    },
    Field {
        def: MappableField {
            key: "latitude",
            label: "Latitude",
            group: "Location",
            kind: FieldKind::Number,
            required: false,
            help: "",
        },
        aliases: &[a("lat"), a("latitude")],
    },
    Field {
        def: MappableField {
            key: "longitude",
            label: "Longitude",
            group: "Location",
            kind: FieldKind::Number,
            required: false,
            help: "",
        },
        aliases: &[a("long"), a("lon"), a("lng"), a("longitude")],
    },
    // ---- Other ----
    Field {
        def: MappableField {
            key: "use_type",
            label: "Use",
            group: "Other",
            kind: FieldKind::Text,
            required: false,
            help: "OPEN, CLOSED, PRIVATE …",
        },
        aliases: &[a("use"), a("usetype"), a("usage")],
    },
    Field {
        def: MappableField {
            key: "operational_status",
            label: "Operational status",
            group: "Other",
            kind: FieldKind::Text,
            required: false,
            help: "",
        },
        aliases: &[a("operationalstatus"), a("opstatus"), a("status")],
    },
    Field {
        def: MappableField {
            key: "notes",
            label: "Notes",
            group: "Other",
            kind: FieldKind::Text,
            required: false,
            help: "",
        },
        aliases: &[a("notes"), a("note"), a("comment"), a("comments"), a("remarks"), a("memo")],
    },
];

/// Lowercase, letters and digits only — so `Output Freq`, `output_freq` and
/// `OUTPUT-FREQ` are one name.
fn normalize(header: &str) -> String {
    header
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

// ============================================================
// Inspection
// ============================================================

/// One column of the file, as the dialog sees it.
#[derive(Debug, Clone, Serialize)]
pub struct CsvColumn {
    pub index: usize,
    /// The header text, verbatim.
    pub header: String,
    /// A few non-empty values from the top of the file, so the operator can see
    /// what is actually in the column before mapping it.
    pub samples: Vec<String>,
}

/// What [`inspect_csv`] found.
#[derive(Debug, Clone, Serialize)]
pub struct CsvInspection {
    /// `Some(label)` when this is a RepeaterBook export and the dedicated
    /// importer should be used instead. The mapper still works on it.
    pub recognized: Option<String>,
    pub columns: Vec<CsvColumn>,
    /// Data rows in the file, not counting the header.
    pub row_count: usize,
    /// The guess: field key → column index. Fields the guesser could not place
    /// are absent.
    pub guess: ColumnMapping,
    /// Every field a column can be mapped onto, in dialog order.
    pub fields: Vec<MappableField>,
}

/// Field key → zero-based column index. Absent means "not mapped".
pub type ColumnMapping = HashMap<String, usize>;

#[tauri::command]
pub async fn inspect_csv(path: String) -> Result<CsvInspection, String> {
    let headers = csv_headers(&path)?;
    let recognized = recognize_csv(&headers).map(|s| s.label().to_string());

    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_path(&path)
        .map_err(|e| format!("Could not open CSV: {e}"))?;

    // Non-empty values per column, from the first SAMPLE_ROWS data rows. Both
    // the shape test and the dialog's examples read from these.
    let mut samples: Vec<Vec<String>> = vec![Vec::new(); headers.len()];
    let mut row_count = 0usize;
    for result in reader.records() {
        let rec = result.estr()?;
        row_count += 1;
        if row_count <= SAMPLE_ROWS {
            for (i, bucket) in samples.iter_mut().enumerate() {
                if let Some(v) = rec.get(i).map(str::trim).filter(|v| !v.is_empty()) {
                    bucket.push(v.to_string());
                }
            }
        }
    }

    let columns: Vec<CsvColumn> = headers
        .iter()
        .enumerate()
        .map(|(index, header)| CsvColumn {
            index,
            header: header.trim().to_string(),
            samples: samples[index].iter().take(SAMPLE_SHOWN).cloned().collect(),
        })
        .collect();

    Ok(CsvInspection {
        recognized,
        guess: guess_mapping(&columns, &samples),
        columns,
        row_count,
        fields: FIELDS.iter().map(|f| f.def.clone()).collect(),
    })
}

/// Does every non-empty sample in this column parse as a number?
///
/// An empty column satisfies neither shape: with nothing to look at, guessing
/// which of two same-named fields it is would be a coin toss, and leaving it
/// unmapped puts the choice in front of the operator instead.
fn column_shape(samples: &[String]) -> Option<Shape> {
    if samples.is_empty() {
        return None;
    }
    Some(if samples.iter().all(|v| v.parse::<f64>().is_ok()) {
        Shape::Numeric
    } else {
        Shape::NonNumeric
    })
}

/// Match each field against the header row.
///
/// A column is claimed at most once, and only on an exact match of a
/// normalised alias — no substring matching. `Tone` would otherwise match
/// `Uplink Tone`, `Downlink Tone` and `Tone Mode` all at once, and a wrong
/// guess that looks confident is worse than a blank the operator must fill in.
fn guess_mapping(columns: &[CsvColumn], samples: &[Vec<String>]) -> ColumnMapping {
    let mut taken: HashSet<usize> = HashSet::new();
    let mut out = ColumnMapping::new();

    // Pass 1: the shape has to agree, so a column whose values settle the
    // question goes to the field those values belong to.
    claim(columns, samples, &mut taken, &mut out, false);
    // Pass 2: whatever is left may claim a column that is *blank in every
    // sampled row*, which proves no shape either way.
    //
    // Leaving those unmapped was worse than picking. CHIRP's `Tone` column
    // holds the tone mode and is empty on a channel with no tone; in a file
    // whose first rows are all tone-free it is empty in every sampled row. Left
    // unmapped, nothing said "no tone" — and CHIRP writes `DtcsCode` on every
    // row whether or not it is live, so the derivation resurrected the inert
    // code and the whole file imported squelched on DCS 023, unable to key.
    //
    // Catalogue order decides, and it is chosen for this: `tone_mode` precedes
    // `ctcss_uplink`, so a blank `Tone` becomes the mode (whose blank cells
    // then read as an explicit "off") rather than a tone frequency that is
    // blank anyway.
    claim(columns, samples, &mut taken, &mut out, true);
    out
}

/// One matching pass. `empty_only` selects the fallback: with it off a
/// shape-qualified alias needs the shape to match, with it on it needs the
/// column to have had nothing to judge.
fn claim(
    columns: &[CsvColumn],
    samples: &[Vec<String>],
    taken: &mut HashSet<usize>,
    out: &mut ColumnMapping,
    empty_only: bool,
) {
    for field in FIELDS {
        if out.contains_key(field.def.key) {
            continue;
        }
        let found = field.aliases.iter().find_map(|alias| {
            columns.iter().position(|c| {
                if taken.contains(&c.index) || normalize(&c.header) != alias.header {
                    return false;
                }
                let shape = column_shape(&samples[c.index]);
                if empty_only {
                    shape.is_none()
                } else {
                    alias.shape == Shape::Any || shape == Some(alias.shape)
                }
            })
        });
        if let Some(index) = found {
            taken.insert(index);
            out.insert(field.def.key.to_string(), index);
        }
    }
}

// ============================================================
// Parsing under a mapping
// ============================================================

#[tauri::command]
pub async fn preview_mapped_csv(
    path: String,
    mapping: ColumnMapping,
) -> Result<ImportPreview, String> {
    Ok(build_preview(&parse_mapped_csv(&path, &mapping)?))
}

#[tauri::command]
pub async fn import_mapped_csv(
    state: State<'_, AppState>,
    path: String,
    mapping: ColumnMapping,
) -> Result<ImportSummary, String> {
    insert_mapped(&state.pool, &parse_mapped_csv(&path, &mapping)?).await
}

/// Parse the whole file under `mapping`.
pub(crate) fn parse_mapped_csv(
    path: &str,
    mapping: &ColumnMapping,
) -> Result<Vec<ParsedChannel>, String> {
    if !mapping.contains_key("rx_freq") {
        return Err("Map a column to the RX frequency before importing.".to_string());
    }

    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_path(path)
        .map_err(|e| format!("Could not open CSV: {e}"))?;

    // Every read goes through here, so an unmapped field and a blank cell are
    // the same thing everywhere: None.
    let cell = |rec: &csv::StringRecord, key: &str| -> Option<String> {
        mapping
            .get(key)
            .and_then(|&i| rec.get(i))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    let mut out = Vec::new();
    for result in reader.records() {
        let rec = result.estr()?;

        // No frequency, no channel. A blank line and a trailing total row both
        // land here, which is why this is a skip and not an error.
        let rx_freq = match parse_freq_cell(cell(&rec, "rx_freq")) {
            Some(f) => f,
            None => continue,
        };

        // TX comes from its own column when there is one. Otherwise it is built
        // from the offset, whose sign comes from the duplex column when the
        // offset itself is unsigned — which is how CHIRP writes it.
        let tx_freq = match parse_freq_cell(cell(&rec, "tx_freq")) {
            Some(f) => Some(f),
            None => offset_to_tx(
                rx_freq,
                parse_freq_cell(cell(&rec, "offset")),
                cell(&rec, "duplex").as_deref(),
            ),
        };

        let callsign = cell(&rec, "callsign").unwrap_or_default();
        let city = cell(&rec, "city");

        // A spelled-out region becomes its postal code, matching what the
        // RepeaterBook importer stores, so the two libraries filter alike.
        let raw_state = cell(&rec, "state");
        // A spelled-out name first, then the postal code, so one file cannot
        // resolve a country on its `Colorado` rows and none on its `CO` rows.
        let region = raw_state.as_deref().and_then(|s| {
            super::rb_regions::lookup(s).or_else(|| super::rb_regions::lookup_code(s))
        });
        let state = match region {
            Some((code, _)) => Some(code.to_string()),
            None => raw_state,
        };
        let country = cell(&rec, "country").or_else(|| region.map(|(_, c)| c.to_string()));

        // Each tone value is read from its own column, independently. Which of
        // them the channel actually uses is the tone mode's job below.
        let ctcss_up = cell(&rec, "ctcss_uplink").as_deref().and_then(parse_leading_f64);
        let ctcss_dn = cell(&rec, "ctcss_downlink").as_deref().and_then(parse_leading_f64);
        let dcs_up = read_dcs(cell(&rec, "dcs_code"));
        let dcs_dn = read_dcs(cell(&rec, "dcs_rx_code"));

        // A *blank* cell in a mapped tone-mode column is an explicit "off",
        // not a missing value to derive around. CHIRP leaves `Tone` empty on a
        // channel with no tone while still writing values into all three tone
        // columns, so deriving there would resurrect an inert DtcsCode as the
        // channel's squelch.
        let explicit_tone_mode = match mapping.contains_key("tone_mode") {
            false => None,
            true => match cell(&rec, "tone_mode") {
                None => Some("off"),
                // A value we do not recognise — CHIRP's reverse-squelch
                // `TSQL-R`, say — is not an "off"; derive from the values.
                Some(v) => norm_tone_mode(&v),
            },
        };

        let tone = Scheme::new(
            explicit_tone_mode,
            cell(&rec, "cross_mode").as_deref().and_then(norm_cross_mode),
            ctcss_up,
            ctcss_dn,
            dcs_up,
            dcs_dn,
        );
        let Scheme { tone_mode, cross_mode, ctcss_uplink, ctcss_downlink, dcs_code, dcs_rx_code } =
            tone;

        let s = finalize(
            &callsign,
            rx_freq,
            tx_freq,
            ctcss_uplink,
            ctcss_downlink,
            city.as_deref(),
            state.as_deref(),
        );

        let mode = cell(&rec, "mode")
            .as_deref()
            .and_then(norm_mode)
            .unwrap_or("FM")
            .to_string();

        // A DMR-only column on a channel that is not DMR is the operator's
        // mistake or a leftover column; storing it would put a colour code on
        // an FM channel, which the manual editor forbids.
        let dmr = mode == "DMR";
        // A name the operator wrote is kept whole — the channel editor puts no
        // limit on the long name either, and each radio driver trims it to its
        // own display width. The short name is the one the editor does cap, at
        // 7. Where a column is unmapped the generated name stands.
        let name_long = cell(&rec, "name_long").unwrap_or(s.name_long);
        let name_short = cell(&rec, "name_short")
            .map(|n| truncate(&n, 7))
            .unwrap_or(s.name_short);

        out.push(ParsedChannel {
            // Not a RepeaterBook record: no synthetic id, so `insert_mapped`
            // stores NULL and the re-import merge can never claim this row.
            repeaterbook_id: String::new(),
            rb_name: s.rb_name,
            name_long,
            name_short,
            callsign,
            rx_freq,
            tx_freq: s.tx_freq,
            offset: s.offset,
            duplex: s.duplex,
            band: s.band,
            mode: mode.clone(),
            tone_mode,
            cross_mode,
            ctcss_uplink,
            ctcss_downlink,
            dcs_code,
            dcs_rx_code,
            covers: SourceColumns { link_nodes: false, operational_status: false, dcs: false },
            dmr_color_code: dmr
                .then(|| cell(&rec, "dmr_color_code").and_then(|v| v.parse::<i64>().ok()))
                .flatten()
                // The manual editor enforces 0-15 (channels.rs
                // `validate_channel_input`); an import must not be the way
                // round it.
                .filter(|cc| (0..=15).contains(cc)),
            dmr_timeslot: dmr
                .then(|| cell(&rec, "dmr_timeslot").and_then(|v| parse_timeslot(&v)))
                .flatten(),
            dmr_talkgroup: dmr
                .then(|| cell(&rec, "dmr_talkgroup").and_then(|v| v.parse::<i64>().ok()))
                .flatten(),
            power: cell(&rec, "power").as_deref().and_then(norm_power).map(str::to_string),
            // The flags follow the mode column, exactly inverting `derive_mode`.
            // A CSV that lists one mode per channel cannot say a repeater is
            // both DMR and YSF, and inventing a second capability from a stray
            // column would put a channel on the radio in the wrong mode.
            dstar_capable: mode == "DSTAR",
            ysf_capable: mode == "YSF",
            nxdn_capable: mode == "NXDN",
            p25_capable: mode == "P25",
            p25_nac: (mode == "P25").then(|| cell(&rec, "p25_nac")).flatten(),
            m17_capable: mode == "M17",
            tetra_capable: false,
            allstar_node: cell(&rec, "allstar_node"),
            echolink_node: cell(&rec, "echolink_node"),
            irlp_node: cell(&rec, "irlp_node"),
            wires_node: cell(&rec, "wires_node"),
            use_type: cell(&rec, "use_type"),
            operational_status: cell(&rec, "operational_status"),
            city,
            county: cell(&rec, "county"),
            state,
            country,
            latitude: cell(&rec, "latitude").as_deref().and_then(parse_leading_f64),
            longitude: cell(&rec, "longitude").as_deref().and_then(parse_leading_f64),
            notes: cell(&rec, "notes"),
        });
    }

    Ok(out)
}

/// Which way an offset applies, read from a duplex cell.
#[derive(PartialEq, Clone, Copy)]
enum Shift {
    Up,
    Down,
    /// The cell says there is no offset at all.
    Simplex,
    /// Blank, `split`, or a word we do not know. All three mean the same thing
    /// here: the offset column cannot be turned into a TX frequency on its own.
    Unknown,
}

fn read_duplex(cell: Option<&str>) -> Shift {
    match cell.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("+") | Some("plus") | Some("up") | Some("positive") => Shift::Up,
        Some("-") | Some("minus") | Some("down") | Some("negative") => Shift::Down,
        Some("none") | Some("simplex") | Some("off") | Some("sx") => Shift::Simplex,
        // Blank, `s`, `split`, or anything unrecognised.
        _ => Shift::Unknown,
    }
}

/// Turn an offset and a duplex sign into a TX frequency.
///
/// The offset column is written both ways in the wild: signed (`-0.6`), and
/// unsigned with the sign in a separate duplex column (CHIRP writes `-` and
/// `0.600000`). A signed offset carries its own direction; an unsigned one
/// takes the duplex column's.
///
/// An unsigned offset with no usable duplex returns `None` rather than assuming
/// a direction. Most 2 m repeaters are minus and the guess would usually be
/// right, but "usually" here means a channel that transmits on the wrong
/// frequency and cannot key its repeater. `None` imports it as simplex, which
/// the preview shows in the Dup column before anything is written.
///
/// `split` is not resolvable from an offset either — the two frequencies of a
/// split are unrelated by definition — so it needs the TX column.
fn offset_to_tx(rx_freq: f64, offset: Option<f64>, duplex: Option<&str>) -> Option<f64> {
    let shift = read_duplex(duplex);
    if shift == Shift::Simplex {
        return None;
    }
    let offset = offset.filter(|o| *o != 0.0)?;
    let signed = if offset < 0.0 {
        offset
    } else {
        match shift {
            Shift::Up => offset,
            Shift::Down => -offset,
            _ => return None,
        }
    };
    // Binary floating point makes 145.11 - 0.6 land on 144.51000000000002.
    // Amateur frequencies are whole hundreds of hertz; round to that.
    Some(((rx_freq + signed) * 10_000.0).round() / 10_000.0)
}

/// The four tone values as one consistent set.
///
/// `tone_mode` is the switch every radio driver reads, so the values it does
/// not name are cleared rather than carried. A channel that squelches on DCS
/// must not also have a CTCSS tone sitting in the database where a later edit
/// or export can pick it up (issue #71 is what that costs).
struct Scheme {
    tone_mode: String,
    cross_mode: String,
    ctcss_uplink: Option<f64>,
    ctcss_downlink: Option<f64>,
    dcs_code: Option<String>,
    dcs_rx_code: Option<String>,
}

impl Scheme {
    /// `explicit` is the mapped tone-mode column when the file has one and it
    /// says something we recognise, `cross` the mapped cross-mode column.
    ///
    /// An explicit mode is authoritative because it has to be: CHIRP writes
    /// `rToneFreq`, `cToneFreq` **and** `DtcsCode` on every channel whether or
    /// not any of them is live, so the values alone cannot tell `Tone` from
    /// `TSQL` from `DTCS`. Only the mode column can, and it also says which of
    /// the three to keep.
    ///
    /// With no mode column the values are all there is, and the scheme is
    /// derived from them exactly as the RepeaterBook importer does it — a side
    /// with both a DCS code and a CTCSS tone reads as DCS, since a channel that
    /// squelches on DCS ignores the CTCSS value.
    fn new(
        explicit: Option<&'static str>,
        cross: Option<&'static str>,
        ctcss_uplink: Option<f64>,
        ctcss_downlink: Option<f64>,
        dcs_code: Option<String>,
        dcs_rx_code: Option<String>,
    ) -> Scheme {
        let side = |dcs: &Option<String>, ctcss: Option<f64>| match dcs {
            Some(code) => RbTone::Dcs(code.clone()),
            None => match ctcss {
                Some(f) => RbTone::Ctcss(f),
                None => RbTone::None,
            },
        };
        let (derived_mode, derived_cross, derived_dcs, derived_dcs_rx) =
            derive_tone_mode_rb(&side(&dcs_code, ctcss_uplink), &side(&dcs_rx_code, ctcss_downlink));

        let Some(mode) = explicit else {
            return Scheme {
                tone_mode: derived_mode,
                cross_mode: derived_cross,
                // Only the side each derived value came from survives.
                ctcss_uplink: dcs_code.is_none().then_some(ctcss_uplink).flatten(),
                ctcss_downlink: dcs_rx_code.is_none().then_some(ctcss_downlink).flatten(),
                dcs_code: derived_dcs,
                dcs_rx_code: derived_dcs_rx,
            };
        };

        let off = Scheme {
            tone_mode: mode.to_string(),
            cross_mode: "Tone->Tone".to_string(),
            ctcss_uplink: None,
            ctcss_downlink: None,
            dcs_code: None,
            dcs_rx_code: None,
        };
        match mode {
            // TX a CTCSS tone, RX open.
            "Tone" => Scheme { ctcss_uplink, ..off },
            // Both halves squelch on the same CTCSS tone. A file that names
            // TSQL but fills only one of the two columns means that one tone
            // both ways — that is what TSQL is.
            "TSQL" => Scheme {
                ctcss_uplink: ctcss_uplink.or(ctcss_downlink),
                ctcss_downlink: ctcss_downlink.or(ctcss_uplink),
                ..off
            },
            // Same code both ways; `dcs_rx_code` stays None by the storage
            // convention `derive_tone_mode_rb` uses.
            "DTCS" => Scheme { dcs_code: dcs_code.or(dcs_rx_code), ..off },
            // The one scheme that mixes: each half is CTCSS, DCS, or nothing,
            // and the cross mode is the only thing that says which. Without a
            // cross-mode column, fall back to what the values derive to.
            "Cross" => {
                let cross = cross.unwrap_or(&derived_cross);
                let (tx, rx) = cross.split_once("->").unwrap_or(("", ""));
                Scheme {
                    cross_mode: cross.to_string(),
                    ctcss_uplink: (tx == "Tone").then_some(ctcss_uplink).flatten(),
                    ctcss_downlink: (rx == "Tone").then_some(ctcss_downlink).flatten(),
                    dcs_code: (tx == "DTCS").then_some(dcs_code).flatten(),
                    dcs_rx_code: (rx == "DTCS").then_some(dcs_rx_code).flatten(),
                    ..off
                }
            }
            // "off", and anything norm_tone_mode ever adds without a rule here.
            _ => off,
        }
    }
}

/// Read a frequency cell in MHz, refusing one whose number was cut short.
///
/// [`parse_leading_f64`] takes the leading run of digits and `.`/`+`/`-` and
/// parses that, which turns a decimal-comma `145,110` into `145.0` — a channel
/// 110 kHz off that looks entirely plausible in a preview of 300 rows. This is
/// the one required field, and everywhere else the module prefers a blank the
/// operator must fill over a confident wrong value, so it does the same here:
/// if the character that stopped the parse is a comma or another digit, the
/// number was cut in half and the cell is refused. A trailing unit (`146.520
/// MHz`) took nothing out of the number and still reads.
fn parse_freq_cell(cell: Option<String>) -> Option<f64> {
    let raw = cell?;
    let raw = raw.trim();
    let end = raw
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(raw.len());
    if raw[end..].starts_with([',', '\'']) || raw[end..].starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    parse_leading_f64(&raw[..end])
}

/// Read a DCS cell as the 3-digit octal code the channels table stores.
///
/// CHIRP writes a bare `023` where RepeaterBook writes `D023`; both are the
/// same code. A value with an 8 or a 9 in it is not octal and is dropped rather
/// than guessed at — that is `parse_rb_tone`'s rule and this defers to it.
fn read_dcs(cell: Option<String>) -> Option<String> {
    let raw = cell?.trim().to_uppercase();
    let prefixed = if raw.starts_with('D') { raw } else { format!("D{raw}") };
    match parse_rb_tone(Some(prefixed)) {
        // There is no DCS code 000 — the standard list starts at 023. `0` is
        // how a spreadsheet spells "no DCS", and zero-padding it to a code
        // would squelch every channel in the file on a tone no repeater sends.
        RbTone::Dcs(code) if code == "000" => None,
        RbTone::Dcs(code) => Some(code),
        _ => None,
    }
}

/// 1 or 2; anything else is not a time slot.
fn parse_timeslot(v: &str) -> Option<i64> {
    v.parse::<i64>().ok().filter(|t| (1..=2).contains(t))
}

/// Normalise a mode cell to one of ours, or `None` to leave it at FM.
///
/// The aliases are the spellings other tools use: CHIRP writes `NFM` for narrow
/// FM (a bandwidth, not a different mode here), `DV` for D-STAR and `DN` for
/// Fusion's digital-narrow.
fn norm_mode(v: &str) -> Option<&'static str> {
    Some(match normalize(v).as_str() {
        "fm" | "nfm" | "fmn" | "wfm" | "analog" | "analogue" => "FM",
        "am" => "AM",
        "usb" => "USB",
        "lsb" => "LSB",
        "cw" => "CW",
        "dmr" | "dmrtier2" => "DMR",
        "dstar" | "dv" | "dstardv" => "DSTAR",
        "ysf" | "fusion" | "systemfusion" | "c4fm" | "dn" => "YSF",
        "nxdn" => "NXDN",
        "p25" | "p25phase1" | "p25phase2" => "P25",
        "m17" => "M17",
        _ => return None,
    })
}

/// Normalise a tone-mode cell to CHIRP's universal scheme, or `None` to derive
/// it from the tones instead. `TSQL-R` and `DTCS-R` are CHIRP's reverse-squelch
/// variants, which nothing in this database models; they fall back to the
/// derivation rather than being stored as a mode no radio driver knows.
fn norm_tone_mode(v: &str) -> Option<&'static str> {
    Some(match normalize(v).as_str() {
        "off" | "none" | "csq" | "no" => "off",
        "tone" | "ctcss" | "toneencode" => "Tone",
        "tsql" | "ctcsssql" | "tonesql" | "tsqlencode" => "TSQL",
        "dtcs" | "dcs" | "dtcssql" => "DTCS",
        "cross" => "Cross",
        _ => return None,
    })
}

/// Normalise a cross-mode cell to one of the eight the channel editor offers.
///
/// Not routed through [`normalize`]: that strips the `-` and the `>`, which are
/// the only thing separating `Tone->Tone` from `->Tone`.
fn norm_cross_mode(v: &str) -> Option<&'static str> {
    let squashed: String = v.chars().filter(|c| !c.is_whitespace()).collect();
    Some(match squashed.to_ascii_uppercase().as_str() {
        "TONE->TONE" => "Tone->Tone",
        "TONE->DTCS" | "TONE->DCS" => "Tone->DTCS",
        "DTCS->TONE" | "DCS->TONE" => "DTCS->Tone",
        "DTCS->DTCS" | "DCS->DCS" => "DTCS->DTCS",
        "->TONE" => "->Tone",
        "->DTCS" | "->DCS" => "->DTCS",
        "DTCS->" | "DCS->" => "DTCS->",
        "TONE->" => "Tone->",
        _ => return None,
    })
}

/// Normalise a power cell to the three levels the channel editor offers.
fn norm_power(v: &str) -> Option<&'static str> {
    Some(match normalize(v).as_str() {
        "high" | "hi" | "h" | "max" => "High",
        "med" | "medium" | "mid" | "m" => "Med",
        "low" | "lo" | "l" | "min" => "Low",
        _ => return None,
    })
}

// ============================================================
// Insert
// ============================================================

/// Insert mapped rows, skipping ones already in the library.
///
/// Deduped on (rx_freq, name, tx_freq), the same rule the native channel-backup
/// restore uses for manual channels. There is deliberately no merge: a
/// RepeaterBook re-import can merge because RepeaterBook is the authority on
/// what it publishes, and every merged field has an `rb_*` snapshot recording
/// what it last said. An operator's own CSV has no such authority and no such
/// snapshot, so a second import would have no way to tell a stale column in the
/// file from an edit the operator made in the app afterwards — and would
/// overwrite the edit. Skipping keeps the app's copy.
async fn insert_mapped(
    pool: &SqlitePool,
    parsed: &[ParsedChannel],
) -> Result<ImportSummary, String> {
    let mut conn = pool.acquire().await.estr()?;
    let mut tx = conn.begin().await.estr()?;

    let mut added = 0usize;
    let mut skipped = 0usize;

    for p in parsed {
        let duplicate = sqlx::query_as::<_, (i64,)>(
            "SELECT id FROM channels
             WHERE rx_freq = ?1
               AND IFNULL(name_long, '') = ?2
               AND IFNULL(tx_freq, -1) = IFNULL(?3, -1)",
        )
        .bind(p.rx_freq)
        .bind(&p.name_long)
        .bind(p.tx_freq)
        .fetch_optional(&mut *tx)
        .await
        .estr()?
        .is_some();
        if duplicate {
            skipped += 1;
            continue;
        }

        sqlx::query(
            r#"
            INSERT INTO channels (
                name_long, name_short, callsign, rx_freq, tx_freq,
                offset, duplex, band, mode, tone_mode, cross_mode,
                dcs_polarity, ctcss_uplink, ctcss_downlink, dcs_code, dcs_rx_code,
                power, dmr_color_code, dmr_timeslot, dmr_talkgroup,
                dstar_capable, ysf_capable, nxdn_capable, p25_capable, p25_nac,
                m17_capable, tetra_capable,
                allstar_node, echolink_node, irlp_node, wires_node,
                use_type, operational_status, service_type,
                city, county, state, country, latitude, longitude, notes,
                source, last_user_edit
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5,
                ?6, ?7, ?8, ?9, ?10, ?11,
                'NN', ?12, ?13, ?14, ?15,
                ?16, ?17, ?18, ?19,
                ?20, ?21, ?22, ?23, ?24,
                ?25, ?26,
                ?27, ?28, ?29, ?30,
                ?31, ?32, 'Amateur',
                ?33, ?34, ?35, ?36, ?37, ?38, ?39,
                'csv', CURRENT_TIMESTAMP
            )
            "#,
        )
        .bind(&p.name_long)
        .bind(&p.name_short)
        .bind(&p.callsign)
        .bind(p.rx_freq)
        .bind(p.tx_freq)
        .bind(p.offset)
        .bind(&p.duplex)
        .bind(&p.band)
        .bind(&p.mode)
        .bind(&p.tone_mode)
        .bind(&p.cross_mode)
        .bind(p.ctcss_uplink)
        .bind(p.ctcss_downlink)
        .bind(&p.dcs_code)
        .bind(&p.dcs_rx_code)
        .bind(&p.power)
        .bind(p.dmr_color_code)
        .bind(p.dmr_timeslot)
        .bind(p.dmr_talkgroup)
        .bind(p.dstar_capable)
        .bind(p.ysf_capable)
        .bind(p.nxdn_capable)
        .bind(p.p25_capable)
        .bind(&p.p25_nac)
        .bind(p.m17_capable)
        .bind(p.tetra_capable)
        .bind(&p.allstar_node)
        .bind(&p.echolink_node)
        .bind(&p.irlp_node)
        .bind(&p.wires_node)
        .bind(&p.use_type)
        .bind(&p.operational_status)
        .bind(&p.city)
        .bind(&p.county)
        .bind(&p.state)
        .bind(&p.country)
        .bind(p.latitude)
        .bind(p.longitude)
        .bind(&p.notes)
        .execute(&mut *tx)
        .await
        .estr()?;

        added += 1;
    }

    tx.commit().await.estr()?;
    Ok(ImportSummary { added, updated: 0, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHIRP: &str = "../sample-data/chirp-sample.csv";
    const CLUB: &str = "../sample-data/generic-club-sample.csv";

    async fn inspect(path: &str) -> CsvInspection {
        inspect_csv(path.to_string()).await.expect("inspect")
    }

    /// The header a column index points at, so a mapping assertion reads as the
    /// column name rather than a number.
    fn mapped<'a>(ins: &'a CsvInspection, field: &str) -> Option<&'a str> {
        ins.guess
            .get(field)
            .map(|&i| ins.columns[i].header.as_str())
    }

    #[tokio::test]
    async fn a_chirp_export_is_not_a_repeaterbook_export() {
        // It has a `Frequency` column, which is all the old router looked for
        // after ruling out the free-tier shape — so every CHIRP file used to be
        // handed to the wide RepeaterBook parser, which found no `Call`, no
        // tones and no location and imported nameless channels.
        assert!(inspect(CHIRP).await.recognized.is_none());
        assert!(inspect(CLUB).await.recognized.is_none());

        let err = super::super::import::parse_repeaterbook_csv(CHIRP)
            .expect_err("must refuse rather than guess");
        assert!(err.contains("not a RepeaterBook export"), "{err}");
    }

    #[tokio::test]
    async fn the_two_repeaterbook_shapes_are_still_recognized() {
        assert_eq!(
            inspect("../sample-data/repeaterbook-standard-sample.csv")
                .await
                .recognized
                .as_deref(),
            Some("RepeaterBook CSV export"),
        );
        assert_eq!(
            inspect("../sample-data/repeaterbook-sample.csv").await.recognized.as_deref(),
            Some("RepeaterBook \"Full Data\" CSV export"),
        );
    }

    #[tokio::test]
    async fn guesses_every_chirp_column_it_has_a_field_for() {
        let ins = inspect(CHIRP).await;
        assert_eq!(mapped(&ins, "rx_freq"), Some("Frequency"));
        assert_eq!(mapped(&ins, "duplex"), Some("Duplex"));
        assert_eq!(mapped(&ins, "offset"), Some("Offset"));
        assert_eq!(mapped(&ins, "name_long"), Some("Name"));
        assert_eq!(mapped(&ins, "mode"), Some("Mode"));
        assert_eq!(mapped(&ins, "power"), Some("Power"));
        assert_eq!(mapped(&ins, "notes"), Some("Comment"));
        assert_eq!(mapped(&ins, "dcs_code"), Some("DtcsCode"));
        assert_eq!(mapped(&ins, "dcs_rx_code"), Some("RxDtcsCode"));

        // CHIRP's `Location` is the memory slot number. Mapping it to City
        // would file every channel in a town called "3".
        assert_eq!(mapped(&ins, "city"), None);
    }

    /// The header `Tone` means two different things in two common files, and
    /// only the values tell them apart.
    #[tokio::test]
    async fn the_tone_header_is_read_by_its_values() {
        // CHIRP: `Tone` holds TSQL/Tone/DTCS, and the frequencies are elsewhere.
        let chirp = inspect(CHIRP).await;
        assert_eq!(mapped(&chirp, "tone_mode"), Some("Tone"));
        assert_eq!(mapped(&chirp, "ctcss_uplink"), Some("rToneFreq"));
        assert_eq!(mapped(&chirp, "ctcss_downlink"), Some("cToneFreq"));

        // A spreadsheet whose `Tone` column holds 100.0 is the TX tone, and
        // there is no mode column at all.
        let dir = std::env::temp_dir().join(format!("cpm_tone_hdr_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("numeric-tone.csv");
        std::fs::write(&path, "Call,Frequency,Tone\nQQ0AAA,145.11,100.0\n").unwrap();
        let ins = inspect(path.to_str().unwrap()).await;
        assert_eq!(mapped(&ins, "ctcss_uplink"), Some("Tone"));
        assert_eq!(mapped(&ins, "tone_mode"), None);
    }

    /// Same for `Offset`: a number is an offset, `+`/`-` is a duplex.
    #[tokio::test]
    async fn the_offset_header_is_read_by_its_values() {
        let chirp = inspect(CHIRP).await;
        assert_eq!(mapped(&chirp, "offset"), Some("Offset"));
        assert_eq!(mapped(&chirp, "duplex"), Some("Duplex"));

        // The free-tier RepeaterBook export writes `+`/`-` under `Offset`. It
        // is recognised and never reaches the mapper, but if the operator
        // opens the mapper on it anyway the guess must not call that an offset
        // in MHz.
        let rb = inspect("../sample-data/repeaterbook-standard-sample.csv").await;
        assert_eq!(mapped(&rb, "duplex"), Some("Offset"));
        assert_eq!(mapped(&rb, "offset"), None);
    }

    fn parse(path: &str, mapping: &ColumnMapping) -> Vec<ParsedChannel> {
        parse_mapped_csv(path, mapping).expect("parse")
    }

    async fn parse_with_guess(path: &str) -> Vec<ParsedChannel> {
        let ins = inspect(path).await;
        parse(path, &ins.guess)
    }

    #[tokio::test]
    async fn an_unsigned_offset_takes_its_sign_from_the_duplex_column() {
        let rows = parse_with_guess(CHIRP).await;
        // 145.110 with `-` and 0.600000 in two separate columns.
        assert_eq!(rows[0].tx_freq, Some(144.51));
        assert_eq!(rows[0].duplex, "-");
        // `+` and 5.000000.
        assert_eq!(rows[3].tx_freq, Some(447.1));
        assert_eq!(rows[3].duplex, "+");
        // A blank duplex with a zero offset is simplex, not a wrong guess.
        assert_eq!(rows[2].tx_freq, None);
        assert_eq!(rows[2].duplex, "none");
    }

    /// An unsigned offset with nothing to give it a direction imports simplex.
    /// Most 2 m repeaters are minus, so a guess would usually be right — and
    /// when it was not, the channel would transmit on the wrong frequency and
    /// fail to key the repeater with nothing on screen to say so.
    #[test]
    fn an_unsigned_offset_with_no_direction_is_not_guessed() {
        assert_eq!(offset_to_tx(145.11, Some(0.6), None), None);
        assert_eq!(offset_to_tx(145.11, Some(0.6), Some("")), None);
        // A signed one needs no help.
        assert_eq!(offset_to_tx(145.11, Some(-0.6), None), Some(144.51));
        // An explicit simplex cell wins over a stray offset value.
        assert_eq!(offset_to_tx(145.11, Some(0.6), Some("simplex")), None);
        // A split's two frequencies have no arithmetic relationship.
        assert_eq!(offset_to_tx(145.11, Some(0.6), Some("split")), None);
    }

    /// CHIRP fills `rToneFreq` and `cToneFreq` on every channel whether either
    /// is live. Carrying the inert one through would put an RX tone on a
    /// TX-tone-only channel that appears nowhere in the file.
    #[tokio::test]
    async fn an_explicit_tone_mode_clears_the_values_it_does_not_use() {
        let rows = parse_with_guess(CHIRP).await;

        // Row 0: Tone, rToneFreq 100.0, cToneFreq 88.5, DtcsCode 023.
        assert_eq!(rows[0].tone_mode, "Tone");
        assert_eq!(rows[0].ctcss_uplink, Some(100.0));
        assert_eq!(rows[0].ctcss_downlink, None, "an inert cToneFreq must not be stored");
        assert_eq!(rows[0].dcs_code, None, "an inert DtcsCode must not be stored");

        // Row 1: TSQL 88.5 both ways.
        assert_eq!(rows[1].tone_mode, "TSQL");
        assert_eq!(rows[1].ctcss_uplink, Some(88.5));
        assert_eq!(rows[1].ctcss_downlink, Some(88.5));
        assert_eq!(rows[1].dcs_code, None);

        // Row 2: no tone mode at all — the whole scheme is off, even though
        // both frequency columns are filled.
        assert_eq!(rows[2].tone_mode, "off");
        assert_eq!(rows[2].ctcss_uplink, None);
        assert_eq!(rows[2].ctcss_downlink, None);

        // Row 3: DTCS 073, and the CTCSS pair goes away.
        assert_eq!(rows[3].tone_mode, "DTCS");
        assert_eq!(rows[3].dcs_code.as_deref(), Some("073"));
        assert_eq!(rows[3].dcs_rx_code, None, "same code both ways is stored once");
        assert_eq!(rows[3].ctcss_uplink, None);
        assert_eq!(rows[3].ctcss_downlink, None);
    }

    /// Cross is the only scheme that mixes CTCSS and DCS, and with all four
    /// values filled the cross-mode column is the only thing that says which
    /// half is which.
    #[tokio::test]
    async fn a_cross_scheme_keeps_only_the_halves_its_cross_mode_names() {
        let ins = inspect(CHIRP).await;
        assert_eq!(mapped(&ins, "cross_mode"), Some("CrossMode"));

        let rows = parse(CHIRP, &ins.guess);
        let x = &rows[6];
        assert_eq!(x.tone_mode, "Cross");
        assert_eq!(x.cross_mode, "Tone->DTCS");
        assert_eq!(x.ctcss_uplink, Some(110.9), "TX half is CTCSS");
        assert_eq!(x.dcs_rx_code.as_deref(), Some("051"), "RX half is DCS");
        assert_eq!(x.ctcss_downlink, None, "cToneFreq is inert under Tone->DTCS");
        assert_eq!(x.dcs_code, None, "DtcsCode is inert under Tone->DTCS");
    }

    /// A tone mode nothing here models must not be read as "no tone". `TSQL-R`
    /// is CHIRP's reverse squelch: the channel does have a tone, and dropping
    /// it would leave a channel that cannot key its repeater. Falling back to
    /// the values gets the tone right and only the reversal wrong.
    #[tokio::test]
    async fn an_unmodelled_tone_mode_falls_back_to_the_values() {
        let rows = parse_with_guess(CHIRP).await;
        let r = &rows[7];
        assert_eq!(r.tone_mode, "DTCS", "DtcsCode 023 is what the values say");
        assert_eq!(r.dcs_code.as_deref(), Some("023"));
    }

    /// A blank cell in a mapped tone-mode column is an explicit "no tone".
    #[tokio::test]
    async fn a_blank_tone_mode_cell_is_off_not_a_derivation() {
        let ins = inspect(CHIRP).await;
        let rows = parse(CHIRP, &ins.guess);
        // Row 2 has an empty `Tone` and a filled `DtcsCode`, `rToneFreq` and
        // `cToneFreq` — CHIRP writes all three regardless.
        assert_eq!(rows[2].tone_mode, "off");
        assert_eq!(rows[2].dcs_code, None);

        // With the tone-mode column unmapped the same row derives DTCS from
        // the code, which is the right answer for a file that has no mode
        // column at all.
        let mut no_mode = ins.guess.clone();
        no_mode.remove("tone_mode");
        assert_eq!(parse(CHIRP, &no_mode)[2].tone_mode, "DTCS");
    }

    /// With no tone-mode column the scheme is derived from the tones, exactly
    /// as the RepeaterBook importer does it.
    #[tokio::test]
    async fn without_a_tone_mode_column_the_tones_derive_the_scheme() {
        let rows = parse_with_guess(CLUB).await;
        // TX 100.0, RX blank.
        assert_eq!(rows[0].tone_mode, "Tone");
        assert_eq!(rows[0].ctcss_uplink, Some(100.0));
        // 131.8 both ways.
        assert_eq!(rows[3].tone_mode, "TSQL");
        assert_eq!(rows[3].ctcss_downlink, Some(131.8));
    }

    #[tokio::test]
    async fn the_mode_column_drives_the_digital_flags() {
        let chirp = parse_with_guess(CHIRP).await;
        // CHIRP's `DV` is D-STAR and `DN` is Fusion; `NFM` is a bandwidth, not
        // a mode of its own here.
        assert_eq!(chirp[4].mode, "DSTAR");
        assert!(chirp[4].dstar_capable);
        assert!(!chirp[4].ysf_capable);
        assert_eq!(chirp[5].mode, "FM");

        let club = parse_with_guess(CLUB).await;
        assert_eq!(club[1].mode, "DMR");
        assert_eq!(club[1].dmr_color_code, Some(7));
        assert_eq!(club[1].dmr_timeslot, Some(2));
        assert_eq!(club[1].dmr_talkgroup, Some(3108));
    }

    /// A colour code on an FM row is a leftover column or a mistake, and the
    /// manual channel editor would not accept it either.
    #[tokio::test]
    async fn dmr_columns_are_ignored_on_a_channel_that_is_not_dmr() {
        let rows = parse_with_guess(CLUB).await;
        let stray = rows.iter().find(|r| r.callsign == "QQ0HHH").expect("stray row");
        assert_eq!(stray.mode, "FM");
        assert_eq!(stray.dmr_color_code, None);
        assert_eq!(stray.dmr_timeslot, None);
        assert_eq!(stray.dmr_talkgroup, None);
    }

    #[tokio::test]
    async fn a_row_with_no_frequency_is_skipped_not_imported_blank() {
        let rows = parse_with_guess(CLUB).await;
        assert_eq!(rows.len(), 5, "the 6 data rows less the one with no frequency");
        assert!(rows.iter().all(|r| r.rx_freq > 0.0));
    }

    #[tokio::test]
    async fn a_spelled_out_state_becomes_its_postal_code() {
        let rows = parse_with_guess(CLUB).await;
        // So a mapped import and a RepeaterBook import filter and sort alike.
        assert!(rows.iter().all(|r| r.state.as_deref() == Some("CO")));
        assert_eq!(rows[0].country.as_deref(), Some("United States"));
    }

    #[tokio::test]
    async fn a_wattage_is_not_a_power_level() {
        let chirp = parse_with_guess(CHIRP).await;
        // CHIRP writes "50W"/"5W" as often as "High"; a wattage is meaningless
        // without knowing the radio, so it is dropped rather than mapped onto
        // one of the three levels.
        assert_eq!(chirp[0].power, None);
        assert_eq!(chirp[3].power.as_deref(), Some("High"));
        assert_eq!(chirp[4].power.as_deref(), Some("Low"));
        assert_eq!(chirp[5].power.as_deref(), Some("Med"));
    }

    #[tokio::test]
    async fn a_name_column_beats_the_generated_name() {
        let rows = parse_with_guess(CLUB).await;
        assert_eq!(rows[0].name_long, "Anytown Repeater");
        // The short name has no column here, so it is still generated.
        assert_eq!(rows[0].name_short, "QQ0AAA");

        // CHIRP has a Name but no Callsign column; the name still comes from
        // the file rather than from an empty callsign.
        let chirp = parse_with_guess(CHIRP).await;
        assert_eq!(chirp[0].name_long, "Anytown");
        assert_eq!(chirp[0].callsign, "");
    }

    #[tokio::test]
    async fn a_mapping_without_an_rx_frequency_is_refused() {
        let mut mapping = ColumnMapping::new();
        mapping.insert("name_long".to_string(), 1);
        let err = parse_mapped_csv(CLUB, &mapping).expect_err("must refuse");
        assert!(err.contains("RX frequency"), "{err}");
    }

    /// The operator's corrections are what actually get used — the guess is
    /// only a starting point.
    #[tokio::test]
    async fn a_corrected_mapping_overrides_the_guess() {
        let ins = inspect(CLUB).await;
        let mut mapping = ins.guess.clone();
        // Guess put Remarks in the notes; the operator says it is the name.
        let remarks = ins.columns.iter().position(|c| c.header == "Remarks").unwrap();
        mapping.insert("name_long".to_string(), remarks);
        mapping.remove("notes");

        let rows = parse(CLUB, &mapping);
        assert_eq!(rows[0].name_long, "club machine");
        assert_eq!(rows[0].notes, None);
    }

    #[tokio::test]
    async fn samples_show_what_is_in_each_column() {
        let ins = inspect(CLUB).await;
        let call = ins.columns.iter().find(|c| c.header == "Callsign").unwrap();
        assert_eq!(call.samples, vec!["QQ0AAA", "QQ0EEE", "QQ0FFF"]);
        assert_eq!(ins.row_count, 6, "data rows, header excluded");
        // A blank cell is not a sample: the CTCSS RX column is filled on one
        // row only.
        let rx_tone = ins.columns.iter().find(|c| c.header == "CTCSS RX").unwrap();
        assert_eq!(rx_tone.samples, vec!["131.8"]);
    }

    /// Write a CSV into a temp dir and return its path.
    fn tmp_csv(tag: &str, body: &str) -> String {
        let dir = std::env::temp_dir().join(format!("cpm_csvmap_{tag}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{tag}.csv"));
        std::fs::write(&path, body).unwrap();
        path.to_str().unwrap().to_string()
    }

    /// A column that is blank in every sampled row cannot prove its shape, and
    /// used to go unmapped entirely. For CHIRP's `Tone` that is the worst
    /// possible outcome: CHIRP writes `DtcsCode` on every row whether or not
    /// it is live, so with no tone-mode column the derivation resurrects the
    /// inert code and every channel imports squelched on DCS 023 — unable to
    /// key its repeater. This is the exact failure the module exists to avoid.
    #[tokio::test]
    async fn a_tone_column_blank_in_every_sampled_row_is_still_the_tone_mode() {
        let mut body = String::from("Location,Name,Frequency,Tone,rToneFreq,cToneFreq,DtcsCode\n");
        for i in 0..30 {
            body.push_str(&format!("{i},Ch {i},146.{:03},,88.5,88.5,023\n", 400 + i));
        }
        let path = tmp_csv("blank_tone", &body);

        let ins = inspect(&path).await;
        assert_eq!(mapped(&ins, "tone_mode"), Some("Tone"));

        let rows = parse(&path, &ins.guess);
        assert_eq!(rows.len(), 30);
        for r in &rows {
            assert_eq!(r.tone_mode, "off", "{} must not be squelched", r.name_long);
            assert_eq!(r.dcs_code, None);
        }
    }

    /// `0` is how a spreadsheet spells "no DCS". Zero-padding it to `000` and
    /// treating that as a code puts DCS squelch on every channel in the file.
    /// There is no DCS code 000 — the standard list starts at 023.
    #[tokio::test]
    async fn a_zero_in_a_dcs_column_is_no_code_not_code_000() {
        assert_eq!(read_dcs(Some("0".to_string())), None);
        assert_eq!(read_dcs(Some("000".to_string())), None);
        assert_eq!(read_dcs(Some("D000".to_string())), None);
        // A real code still reads, with or without the D.
        assert_eq!(read_dcs(Some("023".to_string())).as_deref(), Some("023"));
        assert_eq!(read_dcs(Some("D023".to_string())).as_deref(), Some("023"));

        let path = tmp_csv(
            "zero_dcs",
            "Call,Frequency,DCS\nQQ0AAA,145.11,0\nQQ0BBB,145.13,023\n",
        );
        let ins = inspect(&path).await;
        let rows = parse(&path, &ins.guess);
        assert_eq!(rows[0].tone_mode, "off");
        assert_eq!(rows[0].dcs_code, None);

        // A lone DCS column with no tone-mode column beside it reads as TX DCS
        // with RX open, not as the same code both ways. The code still goes out
        // and keys the repeater; assuming the downlink code as well would mute
        // the operator whenever the repeater sends a different one. This is the
        // RepeaterBook path's own reading of an uplink-only DCS, unchanged.
        assert_eq!(rows[1].tone_mode, "Cross");
        assert_eq!(rows[1].cross_mode, "DTCS->");
        assert_eq!(rows[1].dcs_code.as_deref(), Some("023"));
    }

    /// The required field is the one place a confident wrong value is worst: a
    /// decimal-comma frequency parsed as its integer part imports a channel
    /// 110 kHz off, which looks entirely plausible in a preview of 300 rows.
    #[tokio::test]
    async fn a_frequency_cut_short_by_a_separator_is_refused_not_truncated() {
        let path = tmp_csv(
            "comma_freq",
            "Call,Frequency\nQQ0AAA,\"145,110\"\nQQ0BBB,145.130\nQQ0CCC,146.520 MHz\n",
        );
        let ins = inspect(&path).await;
        let rows = parse(&path, &ins.guess);
        // The decimal-comma row is skipped rather than imported at 145.0; a
        // trailing unit is still fine, since nothing was cut out of the number.
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].rx_freq, 145.13);
        assert_eq!(rows[1].rx_freq, 146.52);
    }

    /// One file must not produce two countries. `Colorado` resolves through the
    /// region table and `CO` did not, so a mixed file imported half its rows
    /// with a country and half without — and country is a filter.
    #[tokio::test]
    async fn a_postal_code_resolves_the_same_country_as_the_spelled_out_name() {
        let rows = parse_with_guess(CLUB).await;
        assert!(rows.iter().all(|r| r.state.as_deref() == Some("CO")));
        for r in &rows {
            assert_eq!(
                r.country.as_deref(),
                Some("United States"),
                "{} disagrees with the rest of the file",
                r.name_long,
            );
        }
    }

    // ============================================================
    // Insert
    // ============================================================
    async fn test_pool(tag: &str) -> SqlitePool {
        let dir = std::env::temp_dir().join(format!("cpm_csvmap_{tag}_{}", std::process::id()));
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        crate::db::init_pool(&db_path).await.expect("init_pool")
    }

    #[tokio::test]
    async fn mapped_channels_are_not_stored_as_repeaterbook_records() {
        let pool = test_pool("provenance").await;
        let rows = parse_with_guess(CLUB).await;
        let summary = insert_mapped(&pool, &rows).await.expect("import");
        assert_eq!(summary.added, 5);

        // No repeaterbook_id means the re-import merge in import.rs — which
        // exists to let a fresh RepeaterBook export correct RepeaterBook data —
        // can never match one of these rows and rewrite it.
        let (source, rbid): (String, Option<String>) =
            sqlx::query_as("SELECT source, repeaterbook_id FROM channels WHERE callsign = 'QQ0AAA'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(source, "csv");
        assert_eq!(rbid, None);
    }

    #[tokio::test]
    async fn importing_the_same_file_twice_adds_nothing() {
        let pool = test_pool("idempotent").await;
        let rows = parse_with_guess(CLUB).await;
        insert_mapped(&pool, &rows).await.expect("first");
        let second = insert_mapped(&pool, &rows).await.expect("second");
        assert_eq!(second.added, 0);
        assert_eq!(second.skipped, 5);
        assert_eq!(second.updated, 0, "a mapped import never merges");

        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM channels")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 5);
    }

    /// The whole point of the skip: an operator's own CSV carries no `rb_*`
    /// snapshot, so a second import cannot tell a stale column in the file from
    /// an edit made in the app afterwards. Keeping the app's copy is the only
    /// answer that never destroys work.
    #[tokio::test]
    async fn a_second_import_does_not_overwrite_an_edit() {
        let pool = test_pool("keeps_edit").await;
        let rows = parse_with_guess(CLUB).await;
        insert_mapped(&pool, &rows).await.expect("first");
        sqlx::query("UPDATE channels SET ctcss_uplink = 123.0 WHERE callsign = 'QQ0AAA'")
            .execute(&pool)
            .await
            .unwrap();

        insert_mapped(&pool, &rows).await.expect("second");
        let (tone,): (Option<f64>,) =
            sqlx::query_as("SELECT ctcss_uplink FROM channels WHERE callsign = 'QQ0AAA'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(tone, Some(123.0));
    }

    /// Every field in the catalogue has to be readable by `parse_mapped_csv`,
    /// or the dialog offers a mapping that silently does nothing.
    #[test]
    fn every_catalogue_field_is_read_by_the_parser() {
        let source = include_str!("csv_map.rs");
        for field in FIELDS {
            assert!(
                source.contains(&format!("\"{}\")", field.def.key)),
                "{} is offered in the dialog but never read out of a row",
                field.def.key,
            );
        }
    }

    /// Within a pass, no header may be claimable by two fields on shape alone —
    /// otherwise the guess would depend on catalogue order rather than on the
    /// values. (Order *is* the tie-break in the empty-column fallback pass, on
    /// purpose; see `guess_mapping`.)
    #[test]
    fn no_two_fields_claim_the_same_header_and_shape() {
        let mut seen: Vec<(&str, Shape, &str)> = Vec::new();
        for field in FIELDS {
            for alias in field.aliases {
                for (header, shape, owner) in &seen {
                    let clash = *header == alias.header
                        && (*shape == Shape::Any
                            || alias.shape == Shape::Any
                            || *shape == alias.shape);
                    assert!(
                        !clash,
                        "`{}` is claimed by both {owner} and {} — the guess would \
                         depend on catalogue order",
                        alias.header, field.def.key,
                    );
                }
                seen.push((alias.header, alias.shape, field.def.key));
            }
        }
    }
}
