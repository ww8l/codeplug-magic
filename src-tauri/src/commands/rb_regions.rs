//! State/province name -> postal code, for the RepeaterBook standard CSV.
//!
//! The premium "Full Data" JSON already carries the two-letter code (`"state":
//! "CO"`), but the free CSV export spells the name out (`Colorado`). Both feed
//! the same synthetic dedupe id — `CALLSIGN|FREQ|STATE|CITY` — so a CSV import
//! that skipped this step would key every channel differently from the JSON
//! import of the same repeater and duplicate the library instead of refreshing
//! it. Verified against a real 750-row export: normalising here reproduced 717
//! of 750 existing ids exactly, the remainder being repeaters genuinely absent
//! from the older JSON snapshot.
//!
//! The country comes from the same table because the CSV has no country column
//! at all, and "United States" is what the JSON importer already stores.

/// Look up a spelled-out state/province name, returning `(postal_code, country)`.
///
/// Matching is case-insensitive and ignores surrounding whitespace. Returns
/// `None` for anything not listed — the caller keeps the raw name rather than
/// guessing, so an unrecognised region still imports, just with a name where a
/// code would normally sit.
pub fn lookup(name: &str) -> Option<(&'static str, &'static str)> {
    let key = name.trim().to_ascii_uppercase();
    TABLE
        .iter()
        .find(|(n, _, _)| *n == key)
        .map(|(_, code, country)| (*code, *country))
}

/// Resolve a two-letter postal code to `(code, country)`.
///
/// [`lookup`] matches spelled-out names only, so a file that writes `Colorado`
/// on one row and `CO` on the next used to get a country on the first and
/// nothing on the second — one import, two countries, and country is a filter.
pub fn lookup_code(code: &str) -> Option<(&'static str, &'static str)> {
    let key = code.trim().to_ascii_uppercase();
    TABLE
        .iter()
        .find(|(_, c, _)| *c == key)
        .map(|(_, code, country)| (*code, *country))
}

const US: &str = "United States";
const CA: &str = "Canada";

/// Names are stored uppercase so the lookup needs no per-entry allocation.
#[rustfmt::skip]
const TABLE: &[(&str, &str, &str)] = &[
    ("ALABAMA", "AL", US),
    ("ALASKA", "AK", US),
    ("ARIZONA", "AZ", US),
    ("ARKANSAS", "AR", US),
    ("CALIFORNIA", "CA", US),
    ("COLORADO", "CO", US),
    ("CONNECTICUT", "CT", US),
    ("DELAWARE", "DE", US),
    ("DISTRICT OF COLUMBIA", "DC", US),
    ("FLORIDA", "FL", US),
    ("GEORGIA", "GA", US),
    ("HAWAII", "HI", US),
    ("IDAHO", "ID", US),
    ("ILLINOIS", "IL", US),
    ("INDIANA", "IN", US),
    ("IOWA", "IA", US),
    ("KANSAS", "KS", US),
    ("KENTUCKY", "KY", US),
    ("LOUISIANA", "LA", US),
    ("MAINE", "ME", US),
    ("MARYLAND", "MD", US),
    ("MASSACHUSETTS", "MA", US),
    ("MICHIGAN", "MI", US),
    ("MINNESOTA", "MN", US),
    ("MISSISSIPPI", "MS", US),
    ("MISSOURI", "MO", US),
    ("MONTANA", "MT", US),
    ("NEBRASKA", "NE", US),
    ("NEVADA", "NV", US),
    ("NEW HAMPSHIRE", "NH", US),
    ("NEW JERSEY", "NJ", US),
    ("NEW MEXICO", "NM", US),
    ("NEW YORK", "NY", US),
    ("NORTH CAROLINA", "NC", US),
    ("NORTH DAKOTA", "ND", US),
    ("OHIO", "OH", US),
    ("OKLAHOMA", "OK", US),
    ("OREGON", "OR", US),
    ("PENNSYLVANIA", "PA", US),
    ("RHODE ISLAND", "RI", US),
    ("SOUTH CAROLINA", "SC", US),
    ("SOUTH DAKOTA", "SD", US),
    ("TENNESSEE", "TN", US),
    ("TEXAS", "TX", US),
    ("UTAH", "UT", US),
    ("VERMONT", "VT", US),
    ("VIRGINIA", "VA", US),
    ("WASHINGTON", "WA", US),
    ("WEST VIRGINIA", "WV", US),
    ("WISCONSIN", "WI", US),
    ("WYOMING", "WY", US),
    // US territories RepeaterBook lists alongside the states.
    ("AMERICAN SAMOA", "AS", US),
    ("GUAM", "GU", US),
    ("NORTHERN MARIANA ISLANDS", "MP", US),
    ("PUERTO RICO", "PR", US),
    ("VIRGIN ISLANDS", "VI", US),
    // Canada: RepeaterBook's other North American region.
    ("ALBERTA", "AB", CA),
    ("BRITISH COLUMBIA", "BC", CA),
    ("MANITOBA", "MB", CA),
    ("NEW BRUNSWICK", "NB", CA),
    ("NEWFOUNDLAND AND LABRADOR", "NL", CA),
    ("NORTHWEST TERRITORIES", "NT", CA),
    ("NOVA SCOTIA", "NS", CA),
    ("NUNAVUT", "NU", CA),
    ("ONTARIO", "ON", CA),
    ("PRINCE EDWARD ISLAND", "PE", CA),
    ("QUEBEC", "QC", CA),
    ("SASKATCHEWAN", "SK", CA),
    ("YUKON", "YT", CA),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_states_a_real_export_contained() {
        // The three states in the exports this parser was built against.
        assert_eq!(lookup("Colorado"), Some(("CO", "United States")));
        assert_eq!(lookup("New Mexico"), Some(("NM", "United States")));
        assert_eq!(lookup("Wyoming"), Some(("WY", "United States")));
    }

    #[test]
    fn matching_ignores_case_and_surrounding_space() {
        assert_eq!(lookup("  colorado ").map(|(c, _)| c), Some("CO"));
        assert_eq!(lookup("NEW YORK").map(|(c, _)| c), Some("NY"));
    }

    #[test]
    fn canadian_provinces_carry_canada_as_the_country() {
        assert_eq!(lookup("Ontario"), Some(("ON", "Canada")));
        assert_eq!(lookup("British Columbia"), Some(("BC", "Canada")));
    }

    #[test]
    fn an_unknown_region_is_none_rather_than_a_guess() {
        assert_eq!(lookup("Bavaria"), None);
        assert_eq!(lookup(""), None);
    }

    #[test]
    fn every_code_is_two_letters_and_unique() {
        let mut codes: Vec<&str> = TABLE.iter().map(|(_, c, _)| *c).collect();
        codes.sort_unstable();
        let before = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), before, "duplicate postal code in the table");
        assert!(TABLE.iter().all(|(_, c, _)| c.len() == 2));
        // Names are pre-uppercased; a lower-case entry would never match.
        assert!(TABLE.iter().all(|(n, _, _)| *n == n.to_ascii_uppercase()));
    }
}
