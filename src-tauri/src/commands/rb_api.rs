//! Searching and importing from the RepeaterBook API (issue #136).
//!
//! **Every limit in this file is a promise.** RepeaterBook approved CodePlug
//! Magic as a distributed app on the strength of the answers in
//! `docs/repeaterbook-api-commitments.md`: how the token is kept, how often the
//! app may ask, how many records it shows and keeps, and how those records stay
//! limited to programming the user's own radios. If a number or a behaviour
//! here has to change, RepeaterBook hears about it first; the doc is not edited
//! to match the code.
//!
//! The shape of the thing:
//!
//! * The token is the user's own `rbuapp_` token, pasted once and kept in the
//!   OS credential store. It is never written to SQLite, settings, logs or a
//!   backup, and it only ever travels in the `X-RB-App-Token` header of an
//!   HTTPS request to RepeaterBook.
//! * Every request goes through [`fetch_export`], which owns the whole rate
//!   policy: one request in flight, 2 s between requests, 20 an hour, the
//!   10-minute in-memory reuse of an identical request, and the 429/5xx
//!   backoff. Nothing else in the app talks to RepeaterBook.
//! * Records land in the channel library with `source = 'repeaterbook_api'`
//!   and an id built from RepeaterBook's own State ID and Repeater ID, so the
//!   export and backup rules elsewhere can tell them from rows imported
//!   out of a RepeaterBook file (`source = 'repeaterbook'`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Acquire, SqlitePool};
use tauri::{AppHandle, Emitter, State};

use super::import::{
    derive_mode, derive_tone_mode_rb, finalize, insert_parsed_tx, jstr, normalize_country,
    parse_bool, parse_leading_f64, parse_rb_tone, synthetic_rb_id, ParsedChannel, RbTone,
    SourceColumns,
};
use super::rb_regions;
use crate::db::AppState;
use crate::error::MapErrString;

// ============================================================
// The promises, as numbers
// ============================================================

/// Sent exactly, on every request, and never changed between releases: the
/// API matches an approved app's User-Agent literally and answers anything else
/// with `403 ua_mismatch`. Deliberately not `http::USER_AGENT`, which carries
/// the version number.
pub(crate) const RB_USER_AGENT: &str =
    "CodePlugMagic (+https://github.com/ww8l/codeplug-magic; WW8L; tim@ww8l.com)";

const NA_URL: &str = "https://www.repeaterbook.com/api/export.php";
const ROW_URL: &str = "https://www.repeaterbook.com/api/exportROW.php";

/// The `source` stamped on every channel that came from the API.
pub(crate) const API_SOURCE: &str = "repeaterbook_api";
/// The `source` of a channel imported from a RepeaterBook CSV/JSON file.
pub(crate) const FILE_SOURCE: &str = "repeaterbook";

/// Either kind of RepeaterBook record, API or file.
pub(crate) fn is_repeaterbook_source(source: &str) -> bool {
    source == API_SOURCE || source == FILE_SOURCE
}

/// Prefix of the `repeaterbook_id` of an API record.
const API_ID_PREFIX: &str = "rbapi:";

/// At most this many records are shown for one search. More than this and
/// nothing is shown or imported; the user narrows the filters instead.
pub(crate) const MAX_RESULTS: usize = 1000;
/// At most this many records selected for one import.
pub(crate) const MAX_SELECTED: usize = 500;

const MIN_SPACING: Duration = Duration::from_secs(2);
const MAX_PER_HOUR: usize = 20;
const HOUR: Duration = Duration::from_secs(3600);
/// An identical request inside this window is answered from memory.
const CACHE_TTL: Duration = Duration::from_secs(600);
/// How many distinct searches the memory cache holds at once. Only a bound on
/// memory; anything older than `CACHE_TTL` is dropped regardless.
const CACHE_ENTRIES: usize = 8;
/// The wait before each retry of a 429/5xx when RepeaterBook sends no
/// Retry-After. Three entries = three retries, then stop.
const BACKOFF_SECS: [u64; 3] = [30, 60, 120];
/// A Retry-After longer than this is honoured by refusing requests until it
/// passes, rather than by holding a search open for that long.
const MAX_INLINE_WAIT: Duration = Duration::from_secs(120);

/// The API stops at 3,500 results. A response that size may be missing
/// records, so filtering it locally could silently leave repeaters out.
const SERVER_TRUNCATION: usize = 3500;
/// Generous for 3,500 records (~1.5 KB each), and a hard stop for anything else.
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

/// Which columns an API record carries: link node numbers, operational status,
/// and tones that can say DCS. See `import::SourceColumns`.
const API_COVERS: SourceColumns =
    SourceColumns { link_nodes: true, operational_status: true, dcs: true };

/// RepeaterBook's own page for creating and managing tokens. Linked from the
/// token setup in Settings.
pub(crate) const TOKENS_PAGE: &str = "https://www.repeaterbook.com/user/api_apps.php";

// ============================================================
// Areas
// ============================================================

/// One search covers one area: a US state or territory, a Canadian province,
/// or one country outside North America.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RbArea {
    Region { state_id: String },
    Country { country: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Endpoint {
    /// `export.php`, token scope `api.export`.
    NorthAmerica,
    /// `exportROW.php`, token scope `api.export_row`.
    RestOfWorld,
}

impl Endpoint {
    fn url(self) -> &'static str {
        match self {
            Endpoint::NorthAmerica => NA_URL,
            Endpoint::RestOfWorld => ROW_URL,
        }
    }
    fn tag(self) -> &'static str {
        match self {
            Endpoint::NorthAmerica => "na",
            Endpoint::RestOfWorld => "row",
        }
    }
    fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "na" => Some(Endpoint::NorthAmerica),
            "row" => Some(Endpoint::RestOfWorld),
            _ => None,
        }
    }
}

/// A searchable North American area, for the area picker.
#[derive(Debug, Clone, Serialize)]
pub struct RbAreaInfo {
    /// RepeaterBook's `state_id`. Not ISO: two-digit FIPS for the US
    /// ("06", not "6"), `CA##` for Canada.
    pub state_id: &'static str,
    pub name: &'static str,
    pub country: &'static str,
}

const US: &str = "United States";
const CA: &str = "Canada";

/// RepeaterBook's `state_id` values. The US uses FIPS; Canada's numbering is
/// RepeaterBook's own and undocumented (provinces alphabetically, then the
/// territories, CA06 unused). Taken from the RepeaterBook Python client's
/// `na_states.py`, which records the same values. Mexico is not offered: the
/// application promised US states, Canadian provinces and countries outside
/// North America.
#[rustfmt::skip]
const NA_AREAS: &[(&str, &str, &str)] = &[
    ("01", "Alabama", US), ("02", "Alaska", US), ("04", "Arizona", US),
    ("05", "Arkansas", US), ("06", "California", US), ("08", "Colorado", US),
    ("09", "Connecticut", US), ("10", "Delaware", US), ("11", "District of Columbia", US),
    ("12", "Florida", US), ("13", "Georgia", US), ("15", "Hawaii", US),
    ("16", "Idaho", US), ("17", "Illinois", US), ("18", "Indiana", US),
    ("19", "Iowa", US), ("20", "Kansas", US), ("21", "Kentucky", US),
    ("22", "Louisiana", US), ("23", "Maine", US), ("24", "Maryland", US),
    ("25", "Massachusetts", US), ("26", "Michigan", US), ("27", "Minnesota", US),
    ("28", "Mississippi", US), ("29", "Missouri", US), ("30", "Montana", US),
    ("31", "Nebraska", US), ("32", "Nevada", US), ("33", "New Hampshire", US),
    ("34", "New Jersey", US), ("35", "New Mexico", US), ("36", "New York", US),
    ("37", "North Carolina", US), ("38", "North Dakota", US), ("39", "Ohio", US),
    ("40", "Oklahoma", US), ("41", "Oregon", US), ("42", "Pennsylvania", US),
    ("44", "Rhode Island", US), ("45", "South Carolina", US), ("46", "South Dakota", US),
    ("47", "Tennessee", US), ("48", "Texas", US), ("49", "Utah", US),
    ("50", "Vermont", US), ("51", "Virginia", US), ("53", "Washington", US),
    ("54", "West Virginia", US), ("55", "Wisconsin", US), ("56", "Wyoming", US),
    ("60", "American Samoa", US), ("66", "Guam", US), ("69", "Northern Mariana Islands", US),
    ("72", "Puerto Rico", US), ("78", "Virgin Islands", US),
    ("CA01", "Alberta", CA), ("CA02", "British Columbia", CA), ("CA03", "Manitoba", CA),
    ("CA04", "New Brunswick", CA), ("CA05", "Newfoundland", CA), ("CA07", "Nova Scotia", CA),
    ("CA08", "Ontario", CA), ("CA09", "Prince Edward Island", CA), ("CA10", "Quebec", CA),
    ("CA11", "Saskatchewan", CA), ("CA12", "Yukon Territory", CA),
    ("CA13", "Northwest Territories", CA), ("CA14", "Nunavut", CA),
];

/// The countries served by the North American endpoint. A country search for
/// one of these goes the wrong way, so it is refused with a pointer to the
/// state/province picker instead.
const NA_COUNTRIES: &[&str] = &["united states", "usa", "us", "canada", "mexico"];

/// A validated area: where the request goes, its one area parameter, and the
/// label used in the log and the UI.
struct ResolvedArea {
    endpoint: Endpoint,
    param: (&'static str, String),
    label: String,
}

fn resolve_area(area: &RbArea) -> Result<ResolvedArea, String> {
    match area {
        RbArea::Region { state_id } => {
            let (id, name, country) = NA_AREAS
                .iter()
                .find(|(id, _, _)| *id == state_id.as_str())
                .ok_or_else(|| format!("\"{state_id}\" is not a state or province this app knows."))?;
            Ok(ResolvedArea {
                endpoint: Endpoint::NorthAmerica,
                param: ("state_id", id.to_string()),
                label: format!("{name}, {country}"),
            })
        }
        RbArea::Country { country } => {
            let c = country.trim();
            if c.is_empty() {
                return Err("Pick a country to search.".to_string());
            }
            if c.chars().count() > 60 {
                return Err("That country name is too long.".to_string());
            }
            if NA_COUNTRIES.contains(&c.to_lowercase().as_str()) {
                return Err(
                    "For the United States and Canada, search one state or province instead of the whole country."
                        .to_string(),
                );
            }
            Ok(ResolvedArea {
                endpoint: Endpoint::RestOfWorld,
                param: ("country", c.to_string()),
                label: c.to_string(),
            })
        }
    }
}

// ============================================================
// Band and mode filters
// ============================================================

/// Amateur bands the search can be narrowed to, as (key, low MHz, high MHz).
/// The API has no band parameter, so this filter runs on the response.
const BANDS: &[(&str, f64, f64)] = &[
    ("10m", 28.0, 29.7),
    ("6m", 50.0, 54.0),
    ("2m", 144.0, 148.0),
    ("1.25m", 219.0, 225.0),
    ("70cm", 420.0, 450.0),
    ("33cm", 902.0, 928.0),
    ("23cm", 1240.0, 1300.0),
];

/// Modes the search can be narrowed to, with the API's own `mode` value where
/// it has one. The API filters only analog, DMR, NXDN, P25 and TETRA; D-STAR
/// and System Fusion are filtered on the response.
/// (our key, the API's `mode` value if it has one, the record's flag field).
type ModeRow = (&'static str, Option<&'static str>, &'static str);

const MODES: &[ModeRow] = &[
    ("FM", Some("analog"), "FM Analog"),
    ("DMR", Some("DMR"), "DMR"),
    ("DSTAR", None, "D-Star"),
    ("YSF", None, "System Fusion"),
    ("NXDN", Some("NXDN"), "NXDN"),
    ("P25", Some("P25"), "APCO P-25"),
];

/// The frequency ranges of the chosen bands. Empty means any band.
fn band_ranges(bands: &[String]) -> Result<Vec<(f64, f64)>, String> {
    bands
        .iter()
        .map(|b| {
            BANDS
                .iter()
                .find(|(k, _, _)| *k == b.as_str())
                .map(|(_, lo, hi)| (*lo, *hi))
                .ok_or_else(|| format!("Unknown band \"{b}\"."))
        })
        .collect()
}

/// The table rows of the chosen modes. Empty means any mode.
fn mode_entries(modes: &[String]) -> Result<Vec<&'static ModeRow>, String> {
    modes
        .iter()
        .map(|m| {
            MODES
                .iter()
                .find(|(k, _, _)| *k == m.as_str())
                .ok_or_else(|| format!("Unknown mode \"{m}\"."))
        })
        .collect()
}

/// The `mode` value to send RepeaterBook, if any. The API takes one value
/// (a repeated parameter keeps only the last), so it is sent only when exactly
/// one mode is chosen and the API knows it; otherwise every mode is filtered
/// here on the response.
fn api_mode_param(modes: &[&'static ModeRow]) -> Option<&'static str> {
    match modes {
        [(_, api, _)] => *api,
        _ => None,
    }
}

// ============================================================
// Parsing one API record
// ============================================================

/// One raw API response, as RepeaterBook sent it.
#[derive(Debug, Deserialize)]
struct RawExport {
    count: Option<Value>,
    /// `null` (not `[]`) when nothing matched, on some endpoints.
    results: Option<Vec<HashMap<String, Value>>>,
}

/// A flag field: RepeaterBook mixes "Yes"/"No" with 1/0.
fn flag(rec: &HashMap<String, Value>, key: &str) -> bool {
    parse_bool(jstr(rec, key))
}

/// Status and Use in the spelling the rest of the library already uses
/// (`On Air`, `Open`), so the facets and filters do not split in two. Anything
/// unrecognised is kept as RepeaterBook wrote it.
fn normalize_status(s: String) -> String {
    match s.to_lowercase().replace(['-', '_'], " ").as_str() {
        "on air" => "On Air".to_string(),
        "off air" => "Off Air".to_string(),
        "unknown" => "Unknown".to_string(),
        _ => s,
    }
}
fn normalize_use(s: String) -> String {
    match s.to_lowercase().as_str() {
        "open" => "Open".to_string(),
        "closed" => "Closed".to_string(),
        "private" => "Private".to_string(),
        _ => s,
    }
}

/// The library id of an API record: endpoint + RepeaterBook's State ID +
/// Repeater ID, which RepeaterBook documents as the pair that identifies a
/// repeater. The endpoint is part of it so a refresh knows where to ask.
fn api_id(endpoint: Endpoint, state_id: &str, rptr_id: &str) -> String {
    format!("{API_ID_PREFIX}{}:{state_id}:{rptr_id}", endpoint.tag())
}

/// Split an API record id back into `(endpoint, state_id, rptr_id)`.
fn split_api_id(id: &str) -> Option<(Endpoint, &str, &str)> {
    let rest = id.strip_prefix(API_ID_PREFIX)?;
    let mut parts = rest.splitn(3, ':');
    let endpoint = Endpoint::from_tag(parts.next()?)?;
    let state_id = parts.next().filter(|s| !s.is_empty())?;
    let rptr_id = parts.next().filter(|s| !s.is_empty())?;
    Some((endpoint, state_id, rptr_id))
}

/// Map one API record into a channel. `None` for a record that has no
/// identifier or no usable output frequency.
fn parse_api_record(rec: &HashMap<String, Value>, endpoint: Endpoint) -> Option<ParsedChannel> {
    let state_id = jstr(rec, "State ID")?;
    let rptr_id = jstr(rec, "Rptr ID")?;
    let rx_freq = jstr(rec, "Frequency")
        .and_then(|s| parse_leading_f64(&s))
        .filter(|f| *f > 0.0)?;
    let tx_freq = jstr(rec, "Input Freq")
        .and_then(|s| parse_leading_f64(&s))
        .filter(|f| *f > 0.0);
    let callsign = jstr(rec, "Callsign").unwrap_or_default();
    let city = jstr(rec, "Nearest City");

    // North American records spell the state out ("Michigan"); the library
    // stores the postal code, as the file importers do, so a repeater keys and
    // filters the same whichever way it arrived. Only for North America: a
    // rest-of-world region that happens to share a name (Georgia) is not a US
    // state, and filing it as one breaks its refresh.
    let raw_state = jstr(rec, "State");
    let raw_country = jstr(rec, "Country").map(normalize_country);
    let na_region = match endpoint {
        Endpoint::NorthAmerica => raw_state.as_deref().and_then(rb_regions::lookup),
        Endpoint::RestOfWorld => None,
    };
    let (state, country) = match na_region {
        Some((code, country)) => (Some(code.to_string()), Some(country.to_string())),
        None => (raw_state.or_else(|| jstr(rec, "Region")), raw_country),
    };

    // PL is the uplink (what you transmit), TSQ the downlink. Either can be a
    // CTCSS frequency, a `Dnnn` DCS code, `CSQ` or blank.
    let up = parse_rb_tone(jstr(rec, "PL"));
    let dn = parse_rb_tone(jstr(rec, "TSQ"));
    let (tone_mode, cross_mode, dcs_code, dcs_rx_code) = derive_tone_mode_rb(&up, &dn);
    let ctcss_uplink = match up {
        RbTone::Ctcss(f) => Some(f),
        _ => None,
    };
    let ctcss_downlink = match dn {
        RbTone::Ctcss(f) => Some(f),
        _ => None,
    };

    let dmr = flag(rec, "DMR");
    let dstar = flag(rec, "D-Star");
    let ysf = flag(rec, "System Fusion");
    let nxdn = flag(rec, "NXDN");
    let p25 = flag(rec, "APCO P-25");
    let m17 = flag(rec, "M17");
    let tetra = flag(rec, "Tetra");
    let dmr_color_code = if dmr {
        jstr(rec, "DMR Color Code")
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|cc| (0..=15).contains(cc))
    } else {
        None
    };
    let p25_nac = if p25 { jstr(rec, "P-25 NAC") } else { None };

    let s = finalize(
        &callsign,
        rx_freq,
        tx_freq,
        ctcss_uplink,
        ctcss_downlink,
        city.as_deref(),
        state.as_deref(),
    );

    let mut note_parts = Vec::new();
    if let Some(lm) = jstr(rec, "Landmark") {
        note_parts.push(format!("Landmark: {lm}"));
    }
    if let Some(n) = jstr(rec, "Notes") {
        note_parts.push(n);
    }
    let notes = (!note_parts.is_empty()).then(|| note_parts.join(" | "));

    Some(ParsedChannel {
        repeaterbook_id: api_id(endpoint, &state_id, &rptr_id),
        rb_name: s.rb_name,
        name_long: s.name_long,
        name_short: s.name_short,
        callsign,
        rx_freq,
        tx_freq: s.tx_freq,
        offset: s.offset,
        duplex: s.duplex,
        band: s.band,
        mode: derive_mode(dmr, dstar, ysf, nxdn, p25, m17),
        // finalize's CTCSS-only reading is replaced by the DCS-aware one.
        tone_mode,
        cross_mode,
        ctcss_uplink,
        ctcss_downlink,
        dcs_code,
        dcs_rx_code,
        dmr_color_code,
        // RepeaterBook has nothing to say about these three.
        dmr_timeslot: None,
        dmr_talkgroup: None,
        power: None,
        dstar_capable: dstar,
        ysf_capable: ysf,
        nxdn_capable: nxdn,
        p25_capable: p25,
        p25_nac,
        m17_capable: m17,
        tetra_capable: tetra,
        allstar_node: jstr(rec, "AllStar Node"),
        echolink_node: jstr(rec, "EchoLink Node"),
        irlp_node: jstr(rec, "IRLP Node"),
        wires_node: jstr(rec, "Wires Node"),
        use_type: jstr(rec, "Use").map(normalize_use),
        operational_status: jstr(rec, "Operational Status").map(normalize_status),
        city,
        county: jstr(rec, "County"),
        state,
        country,
        latitude: jstr(rec, "Lat").and_then(|s| parse_leading_f64(&s)),
        longitude: jstr(rec, "Long").and_then(|s| parse_leading_f64(&s)),
        notes,
        covers: API_COVERS,
    })
}

/// Does a raw record carry `mode`? Uses RepeaterBook's own flag, so an FM
/// search finds every machine with FM analog, mixed-mode ones included.
fn record_has_mode(rec: &HashMap<String, Value>, flag_key: &str) -> bool {
    flag(rec, flag_key)
}

// ============================================================
// The request path: rate limits, memory cache, backoff
// ============================================================

struct CacheEntry {
    key: String,
    at: Instant,
    export: Arc<Vec<HashMap<String, Value>>>,
    truncated: bool,
}

struct Limiter {
    /// The least time between two requests. `MIN_SPACING` in the app; tests
    /// that drive a mock server use zero so they need not sleep.
    min_spacing: Duration,
    in_flight: bool,
    last_request: Option<Instant>,
    /// When each request in the last hour was sent.
    history: VecDeque<Instant>,
    /// Set after a backoff gives up or a long Retry-After: nothing is sent
    /// before this, whatever the user does.
    blocked_until: Option<Instant>,
    /// Set by a 401/403, holding what to tell the user. Nothing is sent until
    /// the token is replaced (or, for an app-identity refusal, the app is
    /// restarted).
    auth_failed: Option<String>,
    cache: Vec<CacheEntry>,
}

impl Limiter {
    fn new(min_spacing: Duration) -> Self {
        Limiter {
            min_spacing,
            in_flight: false,
            last_request: None,
            history: VecDeque::new(),
            blocked_until: None,
            auth_failed: None,
            cache: Vec::new(),
        }
    }

    fn prune(&mut self, now: Instant) {
        while self.history.front().is_some_and(|t| now.duration_since(*t) >= HOUR) {
            self.history.pop_front();
        }
        self.cache.retain(|e| now.duration_since(e.at) < CACHE_TTL);
        if self.blocked_until.is_some_and(|t| t <= now) {
            self.blocked_until = None;
        }
    }

    /// Refuse if the hourly budget is spent, else say how long to wait for the
    /// 2-second spacing.
    fn next_slot(&mut self, now: Instant) -> Result<Duration, String> {
        self.prune(now);
        if self.history.len() >= MAX_PER_HOUR {
            let free = self.history[0] + HOUR;
            let mins = free.saturating_duration_since(now).as_secs().div_ceil(60).max(1);
            return Err(format!(
                "This app sends RepeaterBook at most {MAX_PER_HOUR} requests an hour, and that limit is reached. Try again in about {mins} minute{}.",
                if mins == 1 { "" } else { "s" }
            ));
        }
        Ok(self
            .last_request
            .map(|t| (t + self.min_spacing).saturating_duration_since(now))
            .unwrap_or(Duration::ZERO))
    }

    fn record_request(&mut self, now: Instant) {
        self.last_request = Some(now);
        self.history.push_back(now);
    }

    fn requests_last_hour(&mut self, now: Instant) -> usize {
        self.prune(now);
        self.history.len()
    }
}

static LIMITER: LazyLock<Mutex<Limiter>> = LazyLock::new(|| Mutex::new(Limiter::new(MIN_SPACING)));

fn lock(m: &Mutex<Limiter>) -> std::sync::MutexGuard<'_, Limiter> {
    // A panic while holding the lock leaves plain data behind, never a
    // half-made invariant worth refusing all future searches over.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn limiter() -> std::sync::MutexGuard<'static, Limiter> {
    lock(&LIMITER)
}

/// Clears `in_flight` however the request ends, error and early return
/// included, so one failure can never wedge every later search.
struct InFlight<'a>(&'a Mutex<Limiter>);
impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        lock(self.0).in_flight = false;
    }
}

/// How long to wait before retry number `attempt` (0-based) of a 429/5xx.
fn backoff_delay(attempt: usize, retry_after: Option<Duration>) -> Duration {
    retry_after.unwrap_or_else(|| {
        Duration::from_secs(BACKOFF_SECS[attempt.min(BACKOFF_SECS.len() - 1)])
    })
}

/// Retry-After as seconds or as an HTTP date.
fn parse_retry_after(value: &str) -> Option<Duration> {
    let v = value.trim();
    if let Ok(secs) = v.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let when = chrono::DateTime::parse_from_rfc2822(v).ok()?;
    let secs = (when.timestamp() - chrono::Utc::now().timestamp()).max(0) as u64;
    Some(Duration::from_secs(secs))
}

/// The promised log line: request type, area, HTTP status, timestamp. Nothing
/// else — no token, no URL, no response.
fn log_request(kind: &str, area: &str, status: &str) {
    eprintln!(
        "[repeaterbook] {} {kind} \"{area}\" {status}",
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
    );
}

/// What a 401/403 or an error body means, in words the user can act on.
fn auth_message(error_code: Option<&str>) -> String {
    match error_code {
        Some("auth_scope_denied") => {
            "RepeaterBook says your token is not approved for this part of the world. North America and the rest of the world are approved separately — check the token on RepeaterBook's API Apps & Tokens page.".to_string()
        }
        Some("auth_revoked") | Some("auth_inactive") => {
            "RepeaterBook says this token is no longer active. Generate a new one on RepeaterBook's API Apps & Tokens page and paste it in Settings.".to_string()
        }
        Some("ua_mismatch") => {
            "RepeaterBook did not recognise this app's identity (ua_mismatch). That is a problem in CodePlug Magic, not your token — please report it.".to_string()
        }
        _ => "RepeaterBook did not accept your token. Check it on RepeaterBook's API Apps & Tokens page, then paste it again in Settings.".to_string(),
    }
}

/// The error code in a RepeaterBook error body, if it has one.
fn error_code_of(body: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(body).ok()?;
    v.get("error_code").and_then(Value::as_str).map(str::to_string)
}

/// One API request under every rule in the application. Returns the records,
/// whether RepeaterBook's answer hit its size limit, and whether they came
/// from the 10-minute memory cache.
async fn fetch_export(
    app: &AppHandle,
    token: &str,
    endpoint: Endpoint,
    params: &[(&'static str, String)],
    kind: &str,
    area_label: &str,
) -> Result<(Arc<Vec<HashMap<String, Value>>>, bool, bool), String> {
    let on_wait = |secs: u64| {
        let _ = app.emit("rb-api-wait", secs);
    };
    let transport = Transport {
        limiter: &LIMITER,
        url: endpoint.url(),
        https_only: true,
        on_wait: &on_wait,
    };
    fetch_with(&transport, token, params, kind, area_label).await
}

/// Where a request goes and whose limits it counts against. The app has one;
/// tests build their own around a local mock server and a fresh limiter.
struct Transport<'a> {
    limiter: &'a Mutex<Limiter>,
    url: &'a str,
    https_only: bool,
    /// Told how long a backoff will wait (and 0 when it ends), so the dialog
    /// can count it down.
    on_wait: &'a (dyn Fn(u64) + Send + Sync),
}

/// The body of [`fetch_export`].
///
/// Waiting for the 2-second spacing and for a 429/5xx backoff happens here,
/// inside the one in-flight request, so a second search cannot start while the
/// first is waiting and the wait cannot be skipped by asking again.
async fn fetch_with(
    t: &Transport<'_>,
    token: &str,
    params: &[(&'static str, String)],
    kind: &str,
    area_label: &str,
) -> Result<(Arc<Vec<HashMap<String, Value>>>, bool, bool), String> {
    let key = format!(
        "{}?{}",
        t.url,
        params.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
    );

    let _guard = {
        let mut l = lock(t.limiter);
        let now = Instant::now();
        l.prune(now);
        if let Some(hit) = l.cache.iter().find(|e| e.key == key) {
            return Ok((hit.export.clone(), hit.truncated, true));
        }
        if l.in_flight {
            return Err("A RepeaterBook request is already running. Wait for it to finish.".to_string());
        }
        if let Some(msg) = &l.auth_failed {
            return Err(msg.clone());
        }
        if let Some(until) = l.blocked_until {
            let secs = until.saturating_duration_since(now).as_secs().max(1);
            return Err(format!(
                "RepeaterBook asked this app to slow down. Searching is paused for another {secs} s."
            ));
        }
        l.in_flight = true;
        InFlight(t.limiter)
    };

    let client = reqwest::Client::builder()
        .user_agent(RB_USER_AGENT)
        .https_only(t.https_only)
        // The token rides in a custom header, which a redirect would carry to
        // wherever it points. RepeaterBook has no reason to redirect an API
        // call, so a redirect is treated as an error, never followed.
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let mut attempt = 0usize;
    loop {
        let wait = lock(t.limiter).next_slot(Instant::now())?;
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        lock(t.limiter).record_request(Instant::now());

        let resp = match client
            .get(t.url)
            .query(params)
            .header("X-RB-App-Token", token)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                log_request(kind, area_label, "network-error");
                // `without_url`: the URL holds only the area, but the error
                // text has no need of it.
                return Err(format!("Could not reach RepeaterBook: {}", e.without_url()));
            }
        };
        let status = resp.status();
        log_request(kind, area_label, status.as_str());
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_retry_after);

        if resp.content_length().is_some_and(|n| n as usize > MAX_BODY_BYTES) {
            return Err("RepeaterBook's response was unexpectedly large and was not read.".to_string());
        }
        let body = resp
            .bytes()
            .await
            .map_err(|e| format!("Could not read RepeaterBook's response: {}", e.without_url()))?;
        if body.len() > MAX_BODY_BYTES {
            return Err("RepeaterBook's response was unexpectedly large and was not read.".to_string());
        }
        let code = error_code_of(&body);
        let rate_limited = code.as_deref() == Some("rate_limited");

        if status.as_u16() == 401
            || status.as_u16() == 403
            || code.as_deref().is_some_and(|c| c.starts_with("auth_") || c == "ua_mismatch")
        {
            // Stop at once, no retry — whatever the reason.
            if rate_limited {
                // A refusal for speed, not for the token: pause, as a
                // backoff that gave up would, rather than blame the token.
                let delay = retry_after.unwrap_or(Duration::from_secs(BACKOFF_SECS[BACKOFF_SECS.len() - 1]));
                lock(t.limiter).blocked_until = Some(Instant::now() + delay);
                return Err(slow_down_message(delay));
            }
            // Nothing more is sent until the token is replaced, and every
            // refused search repeats the reason rather than a generic one.
            let msg = auth_message(code.as_deref());
            lock(t.limiter).auth_failed = Some(msg.clone());
            return Err(msg);
        }
        if status.as_u16() == 429 || status.is_server_error() || rate_limited {
            let delay = backoff_delay(attempt, retry_after);
            if attempt >= BACKOFF_SECS.len() || delay > MAX_INLINE_WAIT {
                lock(t.limiter).blocked_until = Some(Instant::now() + delay);
                return Err(slow_down_message(delay));
            }
            (t.on_wait)(delay.as_secs());
            tokio::time::sleep(delay).await;
            (t.on_wait)(0);
            attempt += 1;
            continue;
        }
        if !status.is_success() {
            return Err(format!("RepeaterBook answered HTTP {status}."));
        }
        if code.is_some() {
            let msg = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_else(|| "unknown error".to_string());
            return Err(format!("RepeaterBook returned an error: {msg}"));
        }

        let raw: RawExport = serde_json::from_slice(&body)
            .map_err(|e| format!("RepeaterBook's response could not be read: {e}"))?;
        let results = raw.results.unwrap_or_default();
        let count = raw
            .count
            .as_ref()
            .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
            .unwrap_or(results.len() as u64) as usize;
        let truncated = results.len() >= SERVER_TRUNCATION || count > results.len();
        let export = Arc::new(results);

        let mut l = lock(t.limiter);
        l.cache.push(CacheEntry { key, at: Instant::now(), export: export.clone(), truncated });
        let overflow = l.cache.len().saturating_sub(CACHE_ENTRIES);
        l.cache.drain(..overflow);
        return Ok((export, truncated, false));
    }
}

fn slow_down_message(delay: Duration) -> String {
    let mins = delay.as_secs().div_ceil(60).max(1);
    format!(
        "RepeaterBook is busy or asked this app to slow down, so it stopped trying. Try again in about {mins} minute{}.",
        if mins == 1 { "" } else { "s" }
    )
}

// ============================================================
// The token
// ============================================================

const TOKEN_USER: &str = "api-token";

/// Keychain service name. Taken from the app identifier, so the dev build
/// (`com.ww8l.codeplugmagic.dev`) keeps its own token, as it keeps its own
/// database.
fn token_service(app: &AppHandle) -> String {
    format!("{}.repeaterbook", app.config().identifier)
}

fn token_entry(app: &AppHandle) -> Result<keyring::Entry, String> {
    keyring::Entry::new(&token_service(app), TOKEN_USER)
        .map_err(|e| format!("Could not open the system credential store: {e}"))
}

/// The stored token, or `None` if there is none.
async fn read_token(app: &AppHandle) -> Result<Option<String>, String> {
    let entry = token_entry(app)?;
    tokio::task::spawn_blocking(move || match entry.get_password() {
        Ok(t) => Ok(Some(t)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Could not read the RepeaterBook token from the system credential store: {e}")),
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn delete_token(app: &AppHandle) -> Result<bool, String> {
    let entry = token_entry(app)?;
    tokio::task::spawn_blocking(move || match entry.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(format!("Could not remove the RepeaterBook token: {e}")),
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Check a pasted token's shape. RepeaterBook's per-user app tokens start
/// `rbuapp_`; a shared `app_` token must never be in a distributed app, so one
/// pasted here is refused rather than stored.
fn validate_token(raw: &str) -> Result<String, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err("Paste your RepeaterBook token first.".to_string());
    }
    if t.starts_with("app_") {
        return Err(
            "That is a shared server token (app_…). This app needs your own per-user token, which starts with rbuapp_."
                .to_string(),
        );
    }
    if !t.starts_with("rbuapp_") || t.len() <= "rbuapp_".len() {
        return Err("That doesn't look like a RepeaterBook app token — they start with rbuapp_.".to_string());
    }
    if t.len() > 512 || !t.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("That token has characters a RepeaterBook token cannot contain.".to_string());
    }
    Ok(t.to_string())
}

async fn require_token(app: &AppHandle) -> Result<String, String> {
    read_token(app).await?.ok_or_else(|| {
        "Add your RepeaterBook token in Settings first. Each user creates their own free token on RepeaterBook."
            .to_string()
    })
}

// ============================================================
// Library queries
// ============================================================

async fn library_count(pool: &SqlitePool) -> Result<i64, String> {
    sqlx::query_scalar("SELECT COUNT(*) FROM channels WHERE source = ?1")
        .bind(API_SOURCE)
        .fetch_one(pool)
        .await
        .estr()
}

/// API records not refreshed within 12 months.
pub(crate) const STALE_WHERE: &str = "source = 'repeaterbook_api' \
     AND (last_rb_update IS NULL OR last_rb_update < datetime('now', '-12 months'))";

// ============================================================
// Commands
// ============================================================

#[derive(Debug, Serialize)]
pub struct RbStatus {
    pub has_token: bool,
    pub library_count: i64,
    pub stale_count: i64,
    pub max_results: usize,
    pub max_selected: usize,
    pub requests_last_hour: usize,
    pub requests_per_hour: usize,
    pub tokens_page: &'static str,
}

#[tauri::command]
pub async fn rb_status(app: AppHandle, state: State<'_, AppState>) -> Result<RbStatus, String> {
    let stale_count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM channels WHERE {STALE_WHERE}"))
        .fetch_one(&state.pool)
        .await
        .estr()?;
    Ok(RbStatus {
        has_token: read_token(&app).await?.is_some(),
        library_count: library_count(&state.pool).await?,
        stale_count,
        max_results: MAX_RESULTS,
        max_selected: MAX_SELECTED,
        requests_last_hour: limiter().requests_last_hour(Instant::now()),
        requests_per_hour: MAX_PER_HOUR,
        tokens_page: TOKENS_PAGE,
    })
}

#[tauri::command]
pub async fn rb_areas() -> Vec<RbAreaInfo> {
    NA_AREAS
        .iter()
        .map(|(state_id, name, country)| RbAreaInfo { state_id, name, country })
        .collect()
}

#[tauri::command]
pub async fn rb_set_token(app: AppHandle, token: String) -> Result<(), String> {
    let token = validate_token(&token)?;
    let entry = token_entry(&app)?;
    tokio::task::spawn_blocking(move || entry.set_password(&token))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("Could not save the token in the system credential store: {e}"))?;
    // A new token is the user's answer to a 401/403.
    limiter().auth_failed = None;
    Ok(())
}

#[tauri::command]
pub async fn rb_remove_token(app: AppHandle) -> Result<bool, String> {
    delete_token(&app).await
}

/// One row of a search result.
#[derive(Debug, Serialize)]
pub struct RbResultRow {
    #[serde(flatten)]
    pub channel: ParsedChannel,
    /// `"api"` when this repeater is already in the library from the API (an
    /// import refreshes it), `"file"` when a RepeaterBook file import has it
    /// (an import takes that row over), else `None`.
    pub in_library: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct RbSearchResult {
    pub area: String,
    /// Records matching the search after the band and mode filters.
    pub matched: usize,
    /// More than `MAX_RESULTS` matched: `rows` is empty and nothing can be
    /// imported until the search is narrowed.
    pub too_many: bool,
    /// RepeaterBook's response hit its own size limit, so the band/mode filter
    /// could not be trusted to see every repeater. `rows` is empty.
    pub server_truncated: bool,
    pub from_cache: bool,
    pub rows: Vec<RbResultRow>,
}

#[tauri::command]
pub async fn rb_search(
    app: AppHandle,
    state: State<'_, AppState>,
    area: RbArea,
    filters: RbFilters,
) -> Result<RbSearchResult, String> {
    let resolved = resolve_area(&area)?;
    let bands = band_ranges(&filters.band)?;
    let modes = mode_entries(&filters.mode)?;
    let emcomm = emcomm_flags(&filters.emcomm)?;
    if let Some(n) = &filters.near {
        if !(n.miles > 0.0 && n.miles <= 1000.0) {
            return Err("Pick a distance between 1 and 1,000 miles.".to_string());
        }
    }
    let token = require_token(&app).await?;

    let mut params = vec![resolved.param.clone()];
    if let Some(api_mode) = api_mode_param(&modes) {
        params.push(("mode", api_mode.to_string()));
    }
    let (export, truncated, from_cache) =
        fetch_export(&app, &token, resolved.endpoint, &params, "search", &resolved.label).await?;

    let narrow = Narrowing {
        bands,
        mode_flags: modes.iter().map(|m| m.2).collect(),
        emcomm_flags: emcomm,
        county: lower_trimmed(filters.county.as_deref()),
        city: lower_trimmed(filters.city.as_deref()),
        callsign: lower_trimmed(filters.callsign.as_deref()),
        on_air_only: filters.on_air_only,
        open_only: filters.open_only,
        near: filters.near,
    };
    let rows = filter_records(&export, resolved.endpoint, &narrow);
    // Marked before counting so "hide what I already have" narrows the count
    // the 1,000 limit is judged on, like every other filter.
    let mut rows = mark_in_library(&state.pool, rows).await?;
    if filters.hide_in_library {
        rows.retain(|r| r.in_library.is_none());
    }
    let mut result = RbSearchResult {
        area: resolved.label,
        matched: rows.len(),
        too_many: rows.len() > MAX_RESULTS,
        server_truncated: truncated,
        from_cache,
        rows: Vec::new(),
    };
    if result.too_many || result.server_truncated {
        return Ok(result);
    }
    result.rows = rows;
    Ok(result)
}

/// Ways to narrow a search, all optional. Only the area and the mode reach
/// RepeaterBook (the mode only where the API has a value for it); everything
/// else runs on the response, so changing it within 10 minutes is answered
/// from memory without asking RepeaterBook again.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct RbFilters {
    /// Bands, any of which matches (2 m or 70 cm). Empty means any band.
    pub band: Vec<String>,
    /// Modes, any of which matches (DMR or D-STAR). Empty means any mode.
    pub mode: Vec<String>,
    /// Substring of the county, any case.
    pub county: Option<String>,
    /// Substring of the nearest city.
    pub city: Option<String>,
    /// Substring of the call sign.
    pub callsign: Option<String>,
    /// Only repeaters RepeaterBook lists as on the air.
    pub on_air_only: bool,
    /// Only open repeaters (not closed or private).
    pub open_only: bool,
    /// Any of ARES, RACES, SKYWARN, CANWARN — North America only.
    pub emcomm: Vec<String>,
    /// Within this many miles of a point. Repeaters with no coordinates are
    /// left out, since nothing says how far they are.
    pub near: Option<RbNear>,
    /// Leave out repeaters already in the library, from the API or a file.
    pub hide_in_library: bool,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct RbNear {
    pub latitude: f64,
    pub longitude: f64,
    pub miles: f64,
}

/// [`RbFilters`] after validation, ready to apply to records.
#[derive(Default)]
struct Narrowing {
    bands: Vec<(f64, f64)>,
    mode_flags: Vec<&'static str>,
    emcomm_flags: Vec<&'static str>,
    county: Option<String>,
    city: Option<String>,
    callsign: Option<String>,
    on_air_only: bool,
    open_only: bool,
    near: Option<RbNear>,
}

const EMCOMM: &[&str] = &["ARES", "RACES", "SKYWARN", "CANWARN"];

fn emcomm_flags(v: &[String]) -> Result<Vec<&'static str>, String> {
    v.iter()
        .map(|e| {
            EMCOMM
                .iter()
                .find(|k| k.eq_ignore_ascii_case(e))
                .copied()
                .ok_or_else(|| format!("Unknown emergency service \"{e}\"."))
        })
        .collect()
}

fn lower_trimmed(v: Option<&str>) -> Option<String> {
    v.map(str::trim).filter(|s| !s.is_empty()).map(str::to_lowercase)
}

fn contains_ci(field: Option<&str>, needle: &Option<String>) -> bool {
    match needle {
        None => true,
        Some(n) => field.is_some_and(|f| f.to_lowercase().contains(n.as_str())),
    }
}

/// Great-circle distance in statute miles.
fn haversine_miles(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const EARTH_RADIUS_MI: f64 = 3958.8;
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dlon / 2.0).sin().powi(2);
    EARTH_RADIUS_MI * 2.0 * a.sqrt().asin()
}

fn filter_records(
    export: &[HashMap<String, Value>],
    endpoint: Endpoint,
    n: &Narrowing,
) -> Vec<ParsedChannel> {
    export
        .iter()
        .filter(|rec| n.mode_flags.is_empty() || n.mode_flags.iter().any(|f| record_has_mode(rec, f)))
        .filter(|rec| n.emcomm_flags.is_empty() || n.emcomm_flags.iter().any(|f| flag(rec, f)))
        .filter_map(|rec| parse_api_record(rec, endpoint))
        .filter(|p| n.bands.is_empty() || n.bands.iter().any(|(lo, hi)| p.rx_freq >= *lo && p.rx_freq <= *hi))
        .filter(|p| contains_ci(p.county.as_deref(), &n.county))
        .filter(|p| contains_ci(p.city.as_deref(), &n.city))
        .filter(|p| contains_ci(Some(&p.callsign), &n.callsign))
        .filter(|p| !n.on_air_only || p.operational_status.as_deref() == Some("On Air"))
        .filter(|p| !n.open_only || p.use_type.as_deref() == Some("Open"))
        .filter(|p| {
            n.near.is_none_or(|near| match (p.latitude, p.longitude) {
                (Some(lat), Some(lon)) => {
                    haversine_miles(near.latitude, near.longitude, lat, lon) <= near.miles
                }
                _ => false,
            })
        })
        .collect()
}

async fn mark_in_library(
    pool: &SqlitePool,
    rows: Vec<ParsedChannel>,
) -> Result<Vec<RbResultRow>, String> {
    let existing: Vec<(String, String)> = sqlx::query_as(
        "SELECT repeaterbook_id, source FROM channels \
         WHERE repeaterbook_id IS NOT NULL AND source IN ('repeaterbook', 'repeaterbook_api')",
    )
    .fetch_all(pool)
    .await
    .estr()?;
    let api: HashSet<&str> = existing
        .iter()
        .filter(|(_, s)| s == API_SOURCE)
        .map(|(id, _)| id.as_str())
        .collect();
    let file: HashSet<&str> = existing
        .iter()
        .filter(|(_, s)| s == FILE_SOURCE)
        .map(|(id, _)| id.as_str())
        .collect();
    Ok(rows
        .into_iter()
        .map(|channel| {
            let in_library = if api.contains(channel.repeaterbook_id.as_str()) {
                Some("api")
            } else if file.contains(legacy_id(&channel).as_str()) {
                Some("file")
            } else {
                None
            };
            RbResultRow { channel, in_library }
        })
        .collect())
}

/// The id a RepeaterBook *file* import would have given this repeater.
fn legacy_id(p: &ParsedChannel) -> String {
    synthetic_rb_id(&p.callsign, p.rx_freq, p.state.as_deref(), p.city.as_deref())
}

#[derive(Debug, Serialize)]
pub struct RbImportSummary {
    pub added: usize,
    pub updated: usize,
    /// Rows from a RepeaterBook file import that this import took over. They
    /// are now API records, and are counted in `added` too: for the user they
    /// are new RepeaterBook records.
    pub took_over: usize,
    pub library_count: i64,
}

/// Save the rows the user selected from a search.
#[tauri::command]
pub async fn rb_import(
    state: State<'_, AppState>,
    rows: Vec<ParsedChannel>,
) -> Result<RbImportSummary, String> {
    import_impl(&state.pool, rows).await
}

async fn import_impl(pool: &SqlitePool, rows: Vec<ParsedChannel>) -> Result<RbImportSummary, String> {
    if rows.is_empty() {
        return Err("Select at least one repeater to import.".to_string());
    }
    if rows.len() > MAX_SELECTED {
        return Err(format!(
            "At most {MAX_SELECTED} repeaters can be imported at once; {} are selected.",
            rows.len()
        ));
    }
    let mut seen = HashSet::new();
    let mut rows: Vec<ParsedChannel> = rows
        .into_iter()
        .filter(|r| seen.insert(r.repeaterbook_id.clone()))
        .collect();
    for r in &mut rows {
        if split_api_id(&r.repeaterbook_id).is_none() {
            return Err("Only repeaters from a RepeaterBook search can be imported here.".to_string());
        }
        // `covers` does not survive the trip through the UI; set the truth.
        r.covers = API_COVERS;
    }

    let mut conn = pool.acquire().await.estr()?;
    let mut tx = conn.begin().await.estr()?;

    let existing_api: HashSet<String> = sqlx::query_scalar::<_, String>(
        "SELECT repeaterbook_id FROM channels WHERE source = ?1 AND repeaterbook_id IS NOT NULL",
    )
    .bind(API_SOURCE)
    .fetch_all(&mut *tx)
    .await
    .estr()?
    .into_iter()
    .collect();
    // A repeater already imported from a RepeaterBook file becomes this API
    // record instead of being duplicated beside it. It keeps the operator's
    // names and edits; the merge below refreshes what RepeaterBook owns.
    let mut took_over = 0usize;
    for r in &rows {
        if existing_api.contains(&r.repeaterbook_id) {
            continue;
        }
        took_over += sqlx::query(
            "UPDATE channels SET source = ?1, repeaterbook_id = ?2 \
             WHERE source = 'repeaterbook' AND repeaterbook_id = ?3",
        )
        .bind(API_SOURCE)
        .bind(&r.repeaterbook_id)
        .bind(legacy_id(r))
        .execute(&mut *tx)
        .await
        .estr()?
        .rows_affected() as usize;
    }

    let (added, updated) = insert_parsed_tx(&mut tx, &rows, API_SOURCE).await?;
    tx.commit().await.estr()?;

    Ok(RbImportSummary {
        // A taken-over row went through the merge path, so it is in `updated`;
        // for the user it is a new RepeaterBook record.
        added: added + took_over,
        updated: updated - took_over,
        took_over,
        library_count: library_count(pool).await?,
    })
}

/// Refresh one API record the user asked to refresh. One request, narrowed by
/// the record's area and call sign; the reply is matched on RepeaterBook's own
/// State ID + Repeater ID.
#[tauri::command]
pub async fn rb_refresh_channel(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), String> {
    let (source, rb_id, callsign, country): (String, Option<String>, Option<String>, Option<String>) =
        sqlx::query_as("SELECT source, repeaterbook_id, callsign, country FROM channels WHERE id = ?1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .estr()?
            .ok_or_else(|| "That channel no longer exists.".to_string())?;
    if source != API_SOURCE {
        return Err("Only repeaters imported from the RepeaterBook API can be refreshed.".to_string());
    }
    let rb_id = rb_id.ok_or_else(|| {
        "This is a copy of a RepeaterBook repeater, not the listing itself, so it can't be refreshed. Refresh the original.".to_string()
    })?;
    let (endpoint, state_id, rptr_id) =
        split_api_id(&rb_id).ok_or_else(|| "This record's RepeaterBook id is not recognised.".to_string())?;
    let callsign = callsign
        .filter(|c| !c.trim().is_empty())
        .ok_or_else(|| "This repeater has no call sign to look it up by.".to_string())?;

    let (area_param, label) = match endpoint {
        Endpoint::NorthAmerica => {
            let name = NA_AREAS
                .iter()
                .find(|(sid, _, _)| *sid == state_id)
                .map(|(_, n, _)| *n)
                .unwrap_or(state_id);
            (("state_id", state_id.to_string()), name.to_string())
        }
        Endpoint::RestOfWorld => {
            let c = country
                .filter(|c| !c.trim().is_empty())
                .ok_or_else(|| "This repeater has no country to look it up by.".to_string())?;
            (("country", c.clone()), c)
        }
    };
    let token = require_token(&app).await?;
    let params = vec![area_param, ("callsign", callsign)];
    let (export, _, _) = fetch_export(&app, &token, endpoint, &params, "refresh", &label).await?;

    let fresh = export
        .iter()
        .find(|rec| {
            jstr(rec, "State ID").as_deref() == Some(state_id)
                && jstr(rec, "Rptr ID").as_deref() == Some(rptr_id)
        })
        .and_then(|rec| parse_api_record(rec, endpoint))
        .ok_or_else(|| {
            "RepeaterBook no longer lists this repeater under its call sign. Keep it as it is, or delete it."
                .to_string()
        })?;

    let mut conn = state.pool.acquire().await.estr()?;
    let mut tx = conn.begin().await.estr()?;
    insert_parsed_tx(&mut tx, std::slice::from_ref(&fresh), API_SOURCE).await?;
    tx.commit().await.estr()?;
    Ok(())
}

/// Ids of API records not refreshed within 12 months, for the library's
/// "refresh or delete" prompt.
#[tauri::command]
pub async fn rb_stale_channel_ids(state: State<'_, AppState>) -> Result<Vec<i64>, String> {
    sqlx::query_scalar(&format!("SELECT id FROM channels WHERE {STALE_WHERE} ORDER BY id"))
        .fetch_all(&state.pool)
        .await
        .estr()
}

#[derive(Debug, Serialize)]
pub struct RbRemoved {
    pub channels_deleted: u64,
    pub token_removed: bool,
}

/// "Remove RepeaterBook data": every API record AND the stored token, in one
/// step. Records first — they are the larger promise — then the token, then
/// the memory cache, so nothing RepeaterBook sent survives in the app.
#[tauri::command]
pub async fn rb_remove_all_data(app: AppHandle, state: State<'_, AppState>) -> Result<RbRemoved, String> {
    let channels_deleted = remove_api_records(&state.pool).await?;
    let token_removed = delete_token(&app).await?;
    {
        let mut l = limiter();
        l.cache.clear();
        l.auth_failed = None;
    }
    Ok(RbRemoved { channels_deleted, token_removed })
}

async fn remove_api_records(pool: &SqlitePool) -> Result<u64, String> {
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM channels WHERE source = ?1")
        .bind(API_SOURCE)
        .fetch_all(pool)
        .await
        .estr()?;
    super::channels::delete_impl(pool, &ids).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn test_pool() -> SqlitePool {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("cpm_rb_api_test_{}_{n}", std::process::id()));
        let db_path = dir.join("test.sqlite3");
        let _ = std::fs::remove_file(&db_path);
        crate::db::init_pool(&db_path).await.expect("init_pool")
    }

    /// A North American record in the shape the API returns: the field names
    /// approved clients (dmrfill, the RepeaterBook Python client) decode.
    fn na_record(rptr: u32, call: &str, freq: &str) -> HashMap<String, Value> {
        serde_json::from_value(json!({
            "State ID": "26", "Rptr ID": rptr, "Frequency": freq, "Input Freq": "146.34000",
            "PL": "100.0", "TSQ": "", "Nearest City": "Grand Rapids", "Landmark": "Hill",
            "County": "Kent", "State": "Michigan", "Country": "United States",
            "Lat": "42.96", "Long": "-85.67", "Precise": 1, "Callsign": call,
            "Use": "OPEN", "Operational Status": "On-air", "AllStar Node": "",
            "EchoLink Node": "12345", "IRLP Node": "", "Wires Node": "",
            "FM Analog": "Yes", "DMR": "No", "DMR Color Code": "", "D-Star": "No",
            "NXDN": "No", "APCO P-25": "No", "P-25 NAC": "", "M17": "No", "Tetra": "No",
            "System Fusion": "Yes", "Notes": "Club machine", "Last Update": "2026-01-01"
        }))
        .unwrap()
    }

    #[test]
    fn the_user_agent_is_exactly_what_was_approved() {
        // RepeaterBook matches it literally; a version number here breaks every
        // user's token the day it changes.
        assert_eq!(
            RB_USER_AGENT,
            "CodePlugMagic (+https://github.com/ww8l/codeplug-magic; WW8L; tim@ww8l.com)"
        );
    }

    #[test]
    fn the_promised_numbers() {
        assert_eq!(MAX_RESULTS, 1000);
        assert_eq!(MAX_SELECTED, 500);
        assert_eq!(MIN_SPACING, Duration::from_secs(2));
        assert_eq!(MAX_PER_HOUR, 20);
        assert_eq!(CACHE_TTL, Duration::from_secs(600));
        assert_eq!(BACKOFF_SECS, [30, 60, 120]);
    }

    #[test]
    fn a_token_must_be_a_per_user_app_token() {
        assert_eq!(validate_token("  rbuapp_abc123  ").unwrap(), "rbuapp_abc123");
        assert!(validate_token("").is_err());
        assert!(validate_token("app_shared").unwrap_err().contains("shared"));
        assert!(validate_token("rbuapp_").is_err());
        assert!(validate_token("hello").is_err());
        assert!(validate_token("rbuapp_has space").is_err());
    }

    #[test]
    fn an_api_record_becomes_a_channel() {
        let p = parse_api_record(&na_record(7, "W8ABC", "146.94000"), Endpoint::NorthAmerica).unwrap();
        assert_eq!(p.repeaterbook_id, "rbapi:na:26:7");
        assert_eq!(p.rx_freq, 146.94);
        assert_eq!(p.duplex, "-");
        assert_eq!(p.state.as_deref(), Some("MI"), "state stored as the postal code");
        assert_eq!(p.country.as_deref(), Some("United States"));
        assert_eq!(p.tone_mode, "Tone");
        assert_eq!(p.ctcss_uplink, Some(100.0));
        assert!(p.ysf_capable);
        assert_eq!(p.mode, "YSF");
        assert_eq!(p.operational_status.as_deref(), Some("On Air"));
        assert_eq!(p.use_type.as_deref(), Some("Open"));
        assert_eq!(p.echolink_node.as_deref(), Some("12345"));
        assert_eq!(p.allstar_node, None, "a blank node is no node");
        assert_eq!(p.notes.as_deref(), Some("Landmark: Hill | Club machine"));
        assert_eq!(p.latitude, Some(42.96));
    }

    #[test]
    fn a_dcs_record_keeps_its_code() {
        let mut r = na_record(8, "W8DCS", "443.10000");
        r.insert("PL".into(), json!("D023"));
        r.insert("TSQ".into(), json!("D023"));
        r.insert("Input Freq".into(), json!("448.10000"));
        let p = parse_api_record(&r, Endpoint::NorthAmerica).unwrap();
        assert_eq!(p.tone_mode, "DTCS");
        assert_eq!(p.dcs_code.as_deref(), Some("023"));
    }

    #[test]
    fn a_rest_of_world_region_is_never_read_as_a_us_state() {
        let mut r = na_record(3, "4L1AB", "145.60000");
        r.insert("State ID".into(), json!("GE01"));
        r.insert("State".into(), json!("Georgia"));
        r.insert("Country".into(), json!("Georgia"));
        let p = parse_api_record(&r, Endpoint::RestOfWorld).unwrap();
        assert_eq!(p.state.as_deref(), Some("Georgia"));
        assert_eq!(p.country.as_deref(), Some("Georgia"), "not the United States");
        // The same name from North America is the state.
        let p = parse_api_record(&r, Endpoint::NorthAmerica).unwrap();
        assert_eq!(p.state.as_deref(), Some("GA"));
    }

    #[test]
    fn a_record_without_an_identifier_is_skipped() {
        let mut r = na_record(9, "W8X", "146.94000");
        r.remove("Rptr ID");
        assert!(parse_api_record(&r, Endpoint::NorthAmerica).is_none());
    }

    #[test]
    fn api_ids_round_trip() {
        let id = api_id(Endpoint::RestOfWorld, "GB01", "4321");
        assert_eq!(split_api_id(&id), Some((Endpoint::RestOfWorld, "GB01", "4321")));
        assert_eq!(split_api_id("W0CPH|146.9400|CO|DENVER"), None);
    }

    #[test]
    fn areas_resolve_to_one_parameter() {
        let a = resolve_area(&RbArea::Region { state_id: "26".into() }).unwrap();
        assert_eq!(a.endpoint, Endpoint::NorthAmerica);
        assert_eq!(a.param, ("state_id", "26".to_string()));
        // "6" is not "06": RepeaterBook answers it with nothing, not an error.
        assert!(resolve_area(&RbArea::Region { state_id: "6".into() }).is_err());
        let c = resolve_area(&RbArea::Country { country: " Germany ".into() }).unwrap();
        assert_eq!(c.endpoint, Endpoint::RestOfWorld);
        assert_eq!(c.param, ("country", "Germany".to_string()));
        assert!(resolve_area(&RbArea::Country { country: "Canada".into() }).is_err());
        assert!(resolve_area(&RbArea::Country { country: "".into() }).is_err());
    }

    #[test]
    fn band_and_mode_filters_run_on_the_response() {
        let mut dmr = na_record(2, "W8DMR", "442.50000");
        dmr.insert("DMR".into(), json!("Yes"));
        dmr.insert("Input Freq".into(), json!("447.50000"));
        let recs = vec![na_record(1, "W8FM", "146.94000"), dmr];
        let all = filter_records(&recs, Endpoint::NorthAmerica, &Narrowing::default());
        assert_eq!(all.len(), 2);
        let two_m = filter_records(
            &recs,
            Endpoint::NorthAmerica,
            &Narrowing { bands: band_ranges(&["2m".into()]).unwrap(), ..Default::default() },
        );
        assert_eq!(two_m.len(), 1);
        assert_eq!(two_m[0].callsign, "W8FM");
        let dmr_only = filter_records(
            &recs,
            Endpoint::NorthAmerica,
            &Narrowing { mode_flags: vec!["DMR"], ..Default::default() },
        );
        assert_eq!(dmr_only.len(), 1);
        assert_eq!(dmr_only[0].callsign, "W8DMR");
        // Two choices in one category match either.
        let both_bands = filter_records(
            &recs,
            Endpoint::NorthAmerica,
            &Narrowing { bands: band_ranges(&["2m".into(), "70cm".into()]).unwrap(), ..Default::default() },
        );
        assert_eq!(both_bands.len(), 2);
        let fm_or_dmr = filter_records(
            &recs,
            Endpoint::NorthAmerica,
            &Narrowing { mode_flags: vec!["FM Analog", "DMR"], ..Default::default() },
        );
        assert_eq!(fm_or_dmr.len(), 2);
        // One API-known mode goes to RepeaterBook; D-STAR, or two modes, do not.
        let m = |keys: &[&str]| mode_entries(&keys.iter().map(|k| k.to_string()).collect::<Vec<_>>()).unwrap();
        assert_eq!(api_mode_param(&m(&["DMR"])), Some("DMR"));
        assert_eq!(api_mode_param(&m(&["DSTAR"])), None);
        assert_eq!(api_mode_param(&m(&["DMR", "DSTAR"])), None);
        assert_eq!(api_mode_param(&m(&[])), None);
        assert!(band_ranges(&["4m".into()]).is_err());
    }

    #[test]
    fn the_narrower_filters() {
        let mut off = na_record(2, "K8OFF", "147.00000");
        off.insert("Operational Status".into(), json!("Off-air"));
        off.insert("Use".into(), json!("CLOSED"));
        off.insert("County".into(), json!("Ottawa"));
        off.insert("Nearest City".into(), json!("Holland"));
        off.insert("Lat".into(), json!("42.79"));
        off.insert("Long".into(), json!("-86.11"));
        off.insert("SKYWARN".into(), json!("Yes"));
        let recs = vec![na_record(1, "W8ABC", "146.94000"), off];
        let run = |n: Narrowing| -> Vec<String> {
            filter_records(&recs, Endpoint::NorthAmerica, &n).into_iter().map(|p| p.callsign).collect()
        };
        assert_eq!(run(Narrowing { on_air_only: true, ..Default::default() }), ["W8ABC"]);
        assert_eq!(run(Narrowing { open_only: true, ..Default::default() }), ["W8ABC"]);
        assert_eq!(run(Narrowing { county: lower_trimmed(Some(" otta ")), ..Default::default() }), ["K8OFF"]);
        assert_eq!(run(Narrowing { city: lower_trimmed(Some("grand")), ..Default::default() }), ["W8ABC"]);
        assert_eq!(run(Narrowing { callsign: lower_trimmed(Some("k8")), ..Default::default() }), ["K8OFF"]);
        assert_eq!(run(Narrowing { emcomm_flags: emcomm_flags(&["skywarn".into()]).unwrap(), ..Default::default() }), ["K8OFF"]);
        // Grand Rapids to Holland is about 25 miles.
        let near_gr = |miles| Narrowing {
            near: Some(RbNear { latitude: 42.96, longitude: -85.67, miles }),
            ..Default::default()
        };
        assert_eq!(run(near_gr(10.0)), ["W8ABC"]);
        assert_eq!(run(near_gr(40.0)), ["W8ABC", "K8OFF"]);
        assert!(emcomm_flags(&["CERT".into()]).is_err());
    }

    #[test]
    fn the_hourly_budget_and_spacing_are_enforced() {
        let mut l = Limiter::new(MIN_SPACING);
        let t0 = Instant::now();
        assert_eq!(l.next_slot(t0).unwrap(), Duration::ZERO);
        l.record_request(t0);
        // One second later the 2 s spacing still has a second to run.
        let wait = l.next_slot(t0 + Duration::from_secs(1)).unwrap();
        assert_eq!(wait, Duration::from_secs(1));
        for i in 1..MAX_PER_HOUR as u64 {
            l.record_request(t0 + Duration::from_secs(i * 3));
        }
        let err = l.next_slot(t0 + Duration::from_secs(100)).unwrap_err();
        assert!(err.contains("20 requests an hour"), "{err}");
        // An hour after the first request, one slot frees up.
        assert!(l.next_slot(t0 + HOUR + Duration::from_secs(1)).is_ok());
    }

    // ---- the request path, against a local mock server ----

    type Script = Vec<(u16, Vec<(&'static str, &'static str)>, String)>;

    /// A one-shot HTTP server on localhost: each connection gets the next
    /// scripted response, and the raw request text is kept for inspection.
    async fn mock(script: Script) -> (String, Arc<Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            for (status, headers, body) in script {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = sock.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                log.lock().unwrap().push(String::from_utf8_lossy(&buf).to_string());
                let mut resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                for (k, v) in headers {
                    resp.push_str(&format!("{k}: {v}\r\n"));
                }
                resp.push_str("\r\n");
                resp.push_str(&body);
                sock.write_all(resp.as_bytes()).await.unwrap();
                let _ = sock.shutdown().await;
            }
        });
        (url, seen)
    }

    fn ok_body() -> String {
        json!({ "count": 1, "results": [na_record(1, "W8ABC", "146.94000")] }).to_string()
    }

    async fn fetch(
        lim: &Mutex<Limiter>,
        url: &str,
    ) -> Result<(Arc<Vec<HashMap<String, Value>>>, bool, bool), String> {
        let t = Transport { limiter: lim, url, https_only: false, on_wait: &|_| {} };
        fetch_with(&t, "rbuapp_secret", &[("state_id", "26".to_string())], "search", "Michigan").await
    }

    #[tokio::test]
    async fn the_token_travels_only_in_its_header_and_a_repeat_is_served_from_memory() {
        let (url, seen) = mock(vec![(200, vec![], ok_body())]).await;
        let lim = Mutex::new(Limiter::new(Duration::ZERO));
        let (recs, truncated, cached) = fetch(&lim, &url).await.unwrap();
        assert_eq!((recs.len(), truncated, cached), (1, false, false));

        let req = seen.lock().unwrap()[0].clone();
        let request_line = req.lines().next().unwrap().to_string();
        assert_eq!(request_line, "GET /?state_id=26 HTTP/1.1");
        let lower = req.to_lowercase();
        assert!(lower.contains("x-rb-app-token: rbuapp_secret"), "{req}");
        assert!(
            lower.contains(&format!("user-agent: {}", RB_USER_AGENT.to_lowercase())),
            "{req}"
        );

        let (_, _, cached) = fetch(&lim, &url).await.unwrap();
        assert!(cached, "an identical request inside 10 minutes is answered from memory");
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_429_is_retried_three_times_then_searching_pauses() {
        let busy = |after: &'static str| (429, vec![("Retry-After", after)], "{}".to_string());
        let (url, seen) = mock(vec![busy("0"), busy("0"), busy("0"), busy("60")]).await;
        let lim = Mutex::new(Limiter::new(Duration::ZERO));
        let err = fetch(&lim, &url).await.unwrap_err();
        assert!(err.contains("stopped trying"), "{err}");
        assert_eq!(seen.lock().unwrap().len(), 4, "one request and three retries");
        // The pause holds whatever the user does.
        let err = fetch(&lim, &url).await.unwrap_err();
        assert!(err.contains("paused"), "{err}");
        assert_eq!(seen.lock().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn a_server_error_then_success_recovers() {
        let (url, seen) = mock(vec![
            (503, vec![("Retry-After", "0")], "{}".to_string()),
            (200, vec![], ok_body()),
        ])
        .await;
        let lim = Mutex::new(Limiter::new(Duration::ZERO));
        assert_eq!(fetch(&lim, &url).await.unwrap().0.len(), 1);
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_401_stops_at_once_and_blocks_until_the_token_is_replaced() {
        let body = json!({ "error_code": "auth_invalid", "message": "bad" }).to_string();
        let (url, seen) = mock(vec![(401, vec![], body)]).await;
        let lim = Mutex::new(Limiter::new(Duration::ZERO));
        let err = fetch(&lim, &url).await.unwrap_err();
        assert!(err.contains("did not accept your token"), "{err}");
        let again = fetch(&lim, &url).await.unwrap_err();
        assert_eq!(again, err, "the refusal repeats its own reason");
        assert_eq!(seen.lock().unwrap().len(), 1, "no retry, nothing more sent");
    }

    #[tokio::test]
    async fn a_403_for_rate_is_a_pause_not_a_bad_token() {
        let body = json!({ "error_code": "rate_limited", "message": "slow" }).to_string();
        let (url, seen) = mock(vec![(403, vec![("Retry-After", "30")], body)]).await;
        let lim = Mutex::new(Limiter::new(Duration::ZERO));
        let err = fetch(&lim, &url).await.unwrap_err();
        assert!(err.contains("slow down"), "{err}");
        assert!(lock(&lim).auth_failed.is_none(), "the token was not at fault");
        let err = fetch(&lim, &url).await.unwrap_err();
        assert!(err.contains("paused"), "{err}");
        assert_eq!(seen.lock().unwrap().len(), 1, "a 403 is never retried");
    }

    #[tokio::test]
    async fn an_identity_refusal_says_so_every_time() {
        let body = json!({ "error_code": "ua_mismatch", "message": "ua" }).to_string();
        let (url, _) = mock(vec![(403, vec![], body)]).await;
        let lim = Mutex::new(Limiter::new(Duration::ZERO));
        fetch(&lim, &url).await.unwrap_err();
        let err = fetch(&lim, &url).await.unwrap_err();
        assert!(err.contains("not your token"), "{err}");
    }

    #[test]
    fn backoff_honours_retry_after_else_30_60_120() {
        assert_eq!(backoff_delay(0, None), Duration::from_secs(30));
        assert_eq!(backoff_delay(1, None), Duration::from_secs(60));
        assert_eq!(backoff_delay(2, None), Duration::from_secs(120));
        assert_eq!(backoff_delay(0, Some(Duration::from_secs(7))), Duration::from_secs(7));
        assert_eq!(parse_retry_after("45"), Some(Duration::from_secs(45)));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT"), Some(Duration::ZERO));
        assert_eq!(parse_retry_after("soon"), None);
    }

    #[test]
    fn error_bodies_name_the_problem() {
        assert_eq!(error_code_of(br#"{"error_code":"auth_revoked","message":"x"}"#).as_deref(), Some("auth_revoked"));
        assert_eq!(error_code_of(b"<html>"), None);
        assert!(auth_message(Some("ua_mismatch")).contains("not your token"));
    }

    fn rows(n: u32) -> Vec<ParsedChannel> {
        (0..n)
            .map(|i| {
                let freq = format!("{:.5}", 440.0 + f64::from(i) * 0.0125);
                parse_api_record(&na_record(i, &format!("W8{i:04}"), &freq), Endpoint::NorthAmerica).unwrap()
            })
            .collect()
    }

    #[tokio::test]
    async fn import_stamps_the_api_source_and_refreshes_on_reimport() {
        let pool = test_pool().await;
        let s = import_impl(&pool, rows(3)).await.unwrap();
        assert_eq!((s.added, s.updated, s.library_count), (3, 0, 3));
        let s = import_impl(&pool, rows(3)).await.unwrap();
        assert_eq!((s.added, s.updated, s.library_count), (0, 3, 3), "matched on RepeaterBook's id");
        let src: Vec<String> = sqlx::query_scalar("SELECT DISTINCT source FROM channels")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(src, vec![API_SOURCE.to_string()]);
    }

    #[tokio::test]
    async fn at_most_500_per_import() {
        let pool = test_pool().await;
        let err = import_impl(&pool, rows(501)).await.unwrap_err();
        assert!(err.contains("500"), "{err}");
        assert_eq!(library_count(&pool).await.unwrap(), 0);
    }

    #[tokio::test]
    /// The application's 2,000-record library cap was dropped with
    /// RepeaterBook's agreement (2026-10-10); only the per-search and
    /// per-import limits remain.
    async fn the_library_has_no_total_cap() {
        let pool = test_pool().await;
        let all = rows(2001);
        for chunk in all.chunks(500) {
            import_impl(&pool, chunk.to_vec()).await.unwrap();
        }
        assert_eq!(library_count(&pool).await.unwrap(), 2001);
    }

    #[tokio::test]
    async fn rows_not_from_a_search_are_refused() {
        let pool = test_pool().await;
        let mut r = rows(1);
        r[0].repeaterbook_id = "W0CPH|146.9400|CO|DENVER".into();
        assert!(import_impl(&pool, r).await.is_err());
    }

    #[tokio::test]
    async fn a_file_imported_copy_is_taken_over_not_duplicated() {
        let pool = test_pool().await;
        let r = rows(1);
        sqlx::query(
            "INSERT INTO channels (name_long, callsign, rx_freq, mode, source, repeaterbook_id, state, city) \
             VALUES ('My Name', 'W80000', 440.0, 'FM', 'repeaterbook', ?1, 'MI', 'Grand Rapids')",
        )
        .bind(legacy_id(&r[0]))
        .execute(&pool)
        .await
        .unwrap();
        let s = import_impl(&pool, r).await.unwrap();
        assert_eq!((s.added, s.updated, s.took_over), (1, 0, 1));
        let (n, name, src): (i64, String, String) =
            sqlx::query_as("SELECT COUNT(*), MAX(name_long), MAX(source) FROM channels")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(n, 1);
        assert_eq!(name, "My Name", "the operator's name survives");
        assert_eq!(src, API_SOURCE);
    }

    #[tokio::test]
    async fn remove_all_deletes_only_api_records() {
        let pool = test_pool().await;
        import_impl(&pool, rows(4)).await.unwrap();
        sqlx::query("INSERT INTO channels (name_long, rx_freq, mode, source) VALUES ('Mine', 146.52, 'FM', 'manual')")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(remove_api_records(&pool).await.unwrap(), 4);
        let left: Vec<String> = sqlx::query_scalar("SELECT source FROM channels")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(left, vec!["manual".to_string()]);
    }
}
