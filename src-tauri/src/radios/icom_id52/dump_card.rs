//! Throwaway harness: print what this driver reads out of a real card file, so
//! the decode can be checked against the radio's own screens.
//!
//! Not part of the product. It exists because the settings map has only ever
//! been spot-checked against RT Systems captures, whose values are deliberately
//! scrambled; the first honest check is a file the radio wrote, read by an
//! operator who can walk the menus and confirm each one.
//!
//!     ID52_CARD="/Volumes/NO NAME/ID-52/Setting/Set20260813_01.icf" \
//!         cargo test --lib icom_id52::dump_card -- --ignored --nocapture

#[cfg(test)]
mod tests {
    use serde_json::Value;

    /// Decode a card `.icf` and print every setting under its form section, in
    /// the order the profile editor shows them.
    #[test]
    #[ignore = "needs ID52_CARD=<path to a real .icf>"]
    fn dump() {
        let Ok(path) = std::env::var("ID52_CARD") else {
            eprintln!("set ID52_CARD to a card .icf");
            return;
        };
        let text = std::fs::read_to_string(&path).expect("read the card file");
        let icf = super::super::icf::IcfFile::parse(&text).expect("parses as an ICF");
        super::super::check_is_an_id52_file(&icf).expect("is an ID-52 file this driver understands");

        let decoded = super::super::settings::decode_settings(icf.image());
        let decoded = decoded.as_object().expect("an object");
        let schema: Vec<Value> =
            serde_json::from_str(crate::seed::ID52_SETTINGS_SCHEMA).expect("schema parses");

        println!("\n{path}\n{}", "=".repeat(path.len()));
        for f in &schema {
            let key = f["key"].as_str().unwrap_or_default();
            let label = f["label"].as_str().unwrap_or_default();
            if f["type"] == "section" {
                println!("\n## {label}");
                continue;
            }
            match decoded.get(key) {
                Some(Value::String(s)) => println!("  {label:38} {s}"),
                Some(Value::Bool(b)) => println!("  {label:38} {}", if *b { "ON" } else { "off" }),
                Some(v) => println!("  {label:38} {v}"),
                None => println!("  {label:38} <not decoded>"),
            }
        }
    }
}
