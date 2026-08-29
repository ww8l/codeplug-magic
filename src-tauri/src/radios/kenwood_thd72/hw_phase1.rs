//! THROWAWAY (issue #55, Phase 1): ask the real TH-D72 the questions the
//! published sources cannot answer.
//!
//! Everything below is **read-only**. No `0M PROGRAM`, no block writes, nothing
//! that changes a byte on the radio — this runs before the clone download so a
//! protocol layer that has never met a radio gets its first contact on commands
//! that cannot hurt anything.
//!
//! It deliberately goes through `protocol::command`, the same framing the driver
//! uses, rather than opening its own port and speaking ASCII directly. A harness
//! with its own framing would prove the radio answers something; it would not
//! prove this driver can read it.
//!
//! ```sh
//! CPM_THD72_PORT=/dev/cu.SLAB_USBtoUART \
//!   cargo test --lib kenwood_thd72::hw_phase1 -- --ignored --nocapture
//! ```
//!
//! What each answer settles, from `scratchpad/kenwood_thd72/PLAN.md`:
//!
//! | Command | Decides |
//! |---|---|
//! | `ID` | the model token — `identify` currently accepts anything well-formed |
//! | `TY` | the variant AND whether TX is hardware-extended → `tx_bands` |
//! | `FV 0` | firmware; LA3QMA's tables are V1.10 and commands vanished at V1.08 |
//! | `PV 0..5` | THIS radio's programmable-VFO edges — Menu 130 is user-editable |
//! | `MU` | 19 parameters confirms the published menu sheet; anything else means it is another model's |
//! | `ME`/`MN` | 18 fields, two wider than the TM-D710's 16 |

use super::protocol;

#[test]
#[ignore = "needs a real TH-D72 on the cable"]
fn ask_the_radio_what_the_sources_could_not_say() {
    let port = std::env::var("CPM_THD72_PORT").expect("CPM_THD72_PORT");
    let mut p = protocol::open_port(&port).expect("open the port");

    let mut ask = |cmd: &str| match protocol::command(&mut *p, cmd) {
        Ok(reply) => {
            println!("  {cmd:<10} -> {reply}");
            Some(reply)
        }
        Err(e) => {
            println!("  {cmd:<10} !! {e}");
            None
        }
    };

    println!("\n== identity ==");
    let id = ask("ID");
    ask("TY");
    ask("FV 0");
    ask("AE");

    println!("\n== programmable VFO (the band index every memory must agree with) ==");
    for i in 0..6 {
        ask(&format!("PV {i}"));
    }

    println!("\n== menu ==");
    if let Some(mu) = ask("MU") {
        let fields = mu.trim_start_matches("MU ").split(',').count();
        println!("  MU field count: {fields}  (19 confirms the published sheet)");
    }

    println!("\n== memories ==");
    for n in [0usize, 1, 2, 10, 100] {
        if let Some(me) = ask(&format!("ME {n:03}")) {
            let fields = me.split(',').count();
            println!("       ME {n:03} field count: {fields}  (18 expected; the TM-D710 has 16)");
        }
        ask(&format!("MN {n:03}"));
    }

    assert!(id.is_some(), "the radio did not answer ID — nothing below is trustworthy");
}
