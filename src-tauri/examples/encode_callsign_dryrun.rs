//! DIAGNOSTIC (no hardware): load the real `dmr_users` library, select a
//! prioritized batch exactly like the Tauri command, run `encode_callsign_db`,
//! and print a summary of the RegionPatches (address ranges + sizes) grouped by
//! region. Confirms whether the Contact Map / DB / Limits patches are all
//! generated with sane addresses before we blame the write path.
//!
//! Usage:
//!   cargo run --example encode_callsign_dryrun -- <sqlite-db-path> [max_count]

use ww8l_codeplug_magic_lib::commands::anytone_callsign_db::{
    encode_callsign_db, CallsignDbEntry, DB_BASE, LIMITS_BASE, MAP_BASE,
};

/// One `dmr_users` row as selected below: id, callsign, then the six
/// nullable descriptive columns.
type DmrUserRow = (
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: cargo run --example encode_callsign_dryrun -- <sqlite-db-path> [max_count]");
        std::process::exit(2);
    }
    let db_path = &args[1];
    let max_count: Option<i64> = args.get(2).and_then(|s| s.parse().ok());

    let pool = sqlx::SqlitePool::connect(&format!("sqlite:file:{db_path}?mode=ro"))
        .await
        .expect("open db");

    let limit = max_count.unwrap_or(i64::MAX);
    let rows: Vec<DmrUserRow> =
        sqlx::query_as(
            "SELECT dmr_id, callsign, first_name, last_name, city, state, country, remarks
             FROM dmr_users ORDER BY dmr_id LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&pool)
        .await
        .expect("query");

    let entries: Vec<CallsignDbEntry> = rows
        .into_iter()
        .filter(|r| r.0 > 0 && r.0 <= 99_999_999)
        .map(|(dmr_id, callsign, first, last, city, state, country, remarks)| {
            let name = [first.as_deref(), last.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            CallsignDbEntry {
                dmr_id: dmr_id as u32,
                group_call: false,
                name,
                city: city.unwrap_or_default(),
                call: callsign,
                state: state.unwrap_or_default(),
                country: country.unwrap_or_default(),
                comment: remarks.unwrap_or_default(),
            }
        })
        .collect();
    let user_count = entries.len();

    // Mirror the Tauri command: append the talkgroup library as GROUP-call
    // entries (issue #30). Keep this in step with
    // `commands/program.rs::write_callsign_db` or the dry run stops predicting
    // what actually gets written.
    let tg_rows: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT tg_number, name, call_type FROM talkgroups ORDER BY tg_number")
            .fetch_all(&pool)
            .await
            .expect("talkgroup query");
    let mut entries = entries;
    entries.extend(tg_rows.into_iter().map(|(tg_number, name, call_type)| {
        CallsignDbEntry {
            dmr_id: tg_number.max(0) as u32,
            group_call: !call_type.eq_ignore_ascii_case("private"),
            name,
            city: String::new(),
            call: String::new(),
            state: String::new(),
            country: String::new(),
            comment: String::new(),
        }
    }));

    println!(
        "selected {} entries ({} users + {} talkgroups)",
        entries.len(),
        user_count,
        entries.len() - user_count
    );
    let patches = encode_callsign_db(&entries).expect("encode");
    println!("emitted {} patches\n", patches.len());

    let region = |addr: u32| -> &'static str {
        if addr >= DB_BASE {
            "DB   "
        } else if addr >= MAP_BASE {
            "MAP  "
        } else if addr >= LIMITS_BASE {
            "LIMIT"
        } else {
            "?????"
        }
    };

    let mut by_region: std::collections::BTreeMap<&str, (usize, usize, u32, u32)> =
        std::collections::BTreeMap::new();
    for p in &patches {
        let r = region(p.addr);
        let e = by_region.entry(r).or_insert((0, 0, u32::MAX, 0));
        e.0 += 1;
        e.1 += p.data.len();
        e.2 = e.2.min(p.addr);
        e.3 = e.3.max(p.addr + p.data.len() as u32);
    }
    println!("per-region summary:");
    for (r, (n, bytes, lo, hi)) in &by_region {
        println!("  {r}: {n:>4} patch(es), {bytes:>10} bytes, 0x{lo:08X}..0x{hi:08X}");
    }

    println!("\nfirst 12 patches:");
    for p in patches.iter().take(12) {
        println!("  {} 0x{:08X}  {} bytes", region(p.addr), p.addr, p.data.len());
    }
    println!("last 4 patches:");
    for p in patches.iter().rev().take(4).rev() {
        println!("  {} 0x{:08X}  {} bytes", region(p.addr), p.addr, p.data.len());
    }

    // --- Decode the emitted Contact Map back out and audit the group-call
    //     entries (issue #30). This is the check that matters: the TX screen
    //     resolves a talkgroup through a GCF=1 key, so if none are present the
    //     fix is not in the bytes. Also re-verifies the whole map is strictly
    //     ascending, which the radio's binary search depends on.
    let mut keys: Vec<u32> = Vec::new();
    let mut group_keys: Vec<u32> = Vec::new();
    for p in patches.iter().filter(|p| p.addr >= MAP_BASE && p.addr < DB_BASE) {
        for e in p.data.as_chunks::<8>().0 {
            let key = u32::from_le_bytes([e[0], e[1], e[2], e[3]]);
            keys.push(key);
            if key & 1 == 1 {
                group_keys.push(key);
            }
        }
    }
    let ascending = keys.windows(2).all(|w| w[0] < w[1]);
    println!("\ncontact map audit:");
    println!("  {} entries, strictly ascending: {}", keys.len(), ascending);
    println!("  {} group-call (GCF=1) entries", group_keys.len());
    // Report the group entries as the decimal talkgroup numbers they encode, so
    // the list can be eyeballed against the Talkgroups screen.
    let as_tg = |key: u32| -> u32 {
        let bcd = (key >> 1).to_be_bytes();
        bcd.iter().fold(0u32, |acc, b| acc * 100 + (b >> 4) as u32 * 10 + (b & 0x0F) as u32)
    };
    let shown: Vec<String> = group_keys.iter().take(40).map(|k| as_tg(*k).to_string()).collect();
    println!("  talkgroups: {}", shown.join(", "));
    if !ascending || group_keys.is_empty() {
        eprintln!("\n!! FAILED: map must be ascending and contain group-call entries");
        std::process::exit(1);
    }
}
