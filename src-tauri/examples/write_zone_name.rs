//! Real zone RENAME through the shipping patch-write path (`zone_write_patches`
//! + `run_patch_writes`): backup-before-write, whole-window RMW, single
//! END/commit/reboot. Verify afterwards with read_windows in a fresh session.
//!
//! Usage: cargo run --example write_zone_name -- <serial-port> <zone#> <name> <backup.bin>

use ww8l_codeplug_magic_lib::commands::anytone::{
    run_patch_writes, zone_write_patches, AnytoneZoneEdit, AnytoneZoneWrite,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 {
        eprintln!(
            "usage: cargo run --example write_zone_name -- <serial-port> <zone#> <name> <backup.bin>"
        );
        std::process::exit(2);
    }
    let port = &args[1];
    let zone: usize = args[2].parse().expect("zone must be a positive integer");
    let name = &args[3];
    let backup = std::path::Path::new(&args[4]);

    let patches = zone_write_patches(&[AnytoneZoneWrite {
        zone,
        edit: AnytoneZoneEdit {
            name: Some(name.clone()),
            channel_indices: None,
        },
    }])
    .expect("bad zone edit");

    println!("[write] zone {zone} name → {name:?} (window RMW + backup)…");
    let res = run_patch_writes(port, &patches, backup).expect("patch write failed");
    println!(
        "windows written: {:?}\nbackup: {}\n{}",
        res.windows_written, res.backup_path, res.note
    );
}
