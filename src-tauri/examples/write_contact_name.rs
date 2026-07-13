//! Real contact RENAME through the shipping patch-write path
//! (`contact_write_patches` + `run_patch_writes`): backup-before-write,
//! whole-window RMW, single END/commit/reboot. Verify afterwards with
//! read_windows in a fresh session.
//!
//! Usage: cargo run --example write_contact_name -- <serial-port> <index> <name> <backup.bin>

use ww8l_codeplug_magic_lib::commands::anytone::{
    contact_write_patches, run_patch_writes, AnytoneContactEdit, AnytoneContactWrite,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 {
        eprintln!(
            "usage: cargo run --example write_contact_name -- <serial-port> <index> <name> <backup.bin>"
        );
        std::process::exit(2);
    }
    let port = &args[1];
    let index: usize = args[2].parse().expect("index must be a non-negative integer");
    let name = &args[3];
    let backup = std::path::Path::new(&args[4]);

    let patches = contact_write_patches(&[AnytoneContactWrite {
        index,
        edit: AnytoneContactEdit {
            call_type: None,
            call_alert: None,
            dmr_id: None,
            name: Some(name.clone()),
        },
    }])
    .expect("bad contact edit");

    println!("[write] contact {index} name → {name:?} (window RMW + backup)…");
    let res = run_patch_writes(port, &patches, backup).expect("patch write failed");
    println!(
        "windows written: {:?}\nbackup: {}\n{}",
        res.windows_written, res.backup_path, res.note
    );
}
