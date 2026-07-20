//! BRICK-CAPABLE test writer: mirror the `write_callsign_db` Tauri
//! command from the CLI — pull the first N `dmr_users` (dmr_id ascending),
//! encode with `encode_callsign_db`, and push via `run_patch_writes` in one
//! PC-mode session. Lets us iterate write→read without driving the UI.
//!
//! Usage:
//!   cargo run --example program_callsign_from_db -- <serial-port> <sqlite-db> <count>

use std::path::PathBuf;

use ww8l_codeplug_magic_lib::commands::anytone::{run_patch_writes, run_patch_writes_direct};
use ww8l_codeplug_magic_lib::commands::anytone_callsign_db::{
    encode_callsign_db, CallsignDbEntry, DB_BASE, MAP_BASE,
};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: cargo run --example program_callsign_from_db -- <serial-port> <sqlite-db> <count>");
        std::process::exit(2);
    }
    let port = &args[1];
    let db_path = &args[2];
    let count: i64 = args[3].parse().expect("count");

    let pool = sqlx::SqlitePool::connect(&format!("sqlite:file:{db_path}?mode=ro"))
        .await
        .expect("open db");
    let rows: Vec<(i64, String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as(
            "SELECT dmr_id, callsign, first_name, last_name, city, state, country, remarks
             FROM dmr_users ORDER BY dmr_id LIMIT ?",
        )
        .bind(count)
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
                name,
                city: city.unwrap_or_default(),
                call: callsign,
                state: state.unwrap_or_default(),
                country: country.unwrap_or_default(),
                comment: remarks.unwrap_or_default(),
            }
        })
        .collect();

    let maponly = args.iter().any(|a| a == "maponly");
    let contig = args.iter().any(|a| a == "contig");
    let mut patches = encode_callsign_db(&entries).expect("encode");
    if maponly {
        // Keep only Limits + Map patches (addr < DB_BASE); skip the huge DB write
        // to test whether the map lands on its own at 300k scale.
        patches.retain(|p| p.addr < DB_BASE);
        println!("MAPONLY: keeping {} limits/map patches", patches.len());
    }
    if contig {
        // Merge each region's banks into ONE contiguous patch, filling the
        // inter-bank gaps with 0xFF, so the radio sees a single monotonic write
        // per region (the way CPS pushes a contiguous image). Tests whether the
        // "middle banks drop" quirk is caused by our sparse/gapped layout.
        use ww8l_codeplug_magic_lib::commands::anytone::RegionPatch;
        let merge = |ps: &[RegionPatch]| -> Option<RegionPatch> {
            let lo = ps.iter().map(|p| p.addr).min()?;
            let hi = ps.iter().map(|p| p.addr + p.data.len() as u32).max()?;
            let mut data = vec![0xFFu8; (hi - lo) as usize];
            for p in ps {
                let off = (p.addr - lo) as usize;
                data[off..off + p.data.len()].copy_from_slice(&p.data);
            }
            Some(RegionPatch { addr: lo, data })
        };
        let map: Vec<RegionPatch> = patches.iter().filter(|p| p.addr >= MAP_BASE && p.addr < DB_BASE).cloned().collect();
        let db: Vec<RegionPatch> = patches.iter().filter(|p| p.addr >= DB_BASE).cloned().collect();
        let limits: Vec<RegionPatch> = patches.iter().filter(|p| p.addr < MAP_BASE).cloned().collect();
        let mut merged = Vec::new();
        merged.extend(limits);
        if let Some(m) = merge(&map) { merged.push(m); }
        if let Some(m) = merge(&db) { merged.push(m); }
        patches = merged;
        println!("CONTIG: merged to {} contiguous patches", patches.len());
    }
    println!("writing {} entries in {} patches", entries.len(), patches.len());
    let total: usize = patches.iter().map(|p| p.data.len()).sum();
    println!("total {total} bytes across regions");

    if args.iter().any(|a| a == "writeonly") {
        let r = run_patch_writes_direct(port, &patches).expect("writeonly failed");
        println!("\nWRITEONLY: wrote {} patches (no RMW read, no backup), committed + rebooted.", r.windows_written.len());
        return;
    }

    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup_path = PathBuf::from("/tmp").join(format!("anytone-callsigndb-clitest-{stamp}.bin"));
    let result = run_patch_writes(port, &patches, &backup_path).expect("write failed");
    println!("\nwindows_written: {}", result.windows_written.len());
    println!("backup: {}", result.backup_path);
    println!("{}", result.note);
}
