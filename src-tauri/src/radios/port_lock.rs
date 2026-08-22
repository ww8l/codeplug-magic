//! One radio operation at a time, per serial port (#67).
//!
//! Before this there was no `Mutex`, `RwLock` or `AtomicBool` anywhere in the
//! crate: `AppState` held only the sqlx pool, and the sole thing stopping two
//! radio operations overlapping was per-dialog React `busy` state — which does
//! not survive the dialog being dismissed (#65) and does not coordinate between
//! two dialogs or two windows.
//!
//! It matters more here than it usually would. The AnyTone commits on `END` and
//! then REBOOTS, dropping and re-enumerating its USB port; a second operation
//! that started meanwhile is talking to a port that is about to disappear, in
//! the middle of a flash write. The project has documented "one radio operation
//! per process" since the AnyTone work; this is that rule, enforced.
//!
//! ## Why a plain `std::sync::Mutex` and not an async one
//!
//! The mutex is only ever held for the insert or the remove — never across an
//! `.await`. What lasts for the operation is the [`PortGuard`], which holds no
//! lock at all, just a name in a set. So there is nothing for an async mutex to
//! buy, and a blocking `lock()` here cannot stall the runtime.
//!
//! ## Per-port, not global
//!
//! A global lock would be simpler and would also refuse a genuine two-radio
//! setup, which is a real thing operators have. The hazard is per-port anyway —
//! it is one port's re-enumeration that breaks one operation.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

static BUSY: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// A claim on one port, released when dropped.
///
/// Drop runs on unwind too, so a panic inside a radio operation frees the port
/// rather than wedging it for the life of the process.
#[derive(Debug)]
pub struct PortGuard {
    port: String,
}

impl Drop for PortGuard {
    fn drop(&mut self) {
        // Never poison-propagate here: failing to release would wedge the port
        // permanently, which is strictly worse than whatever panicked.
        let mut busy = BUSY.lock().unwrap_or_else(|e| e.into_inner());
        busy.remove(&self.port);
    }
}

/// Claim `port` for a radio operation, or explain that one is already running.
///
/// Hold the returned guard for the WHOLE operation — move it into the
/// `spawn_blocking` closure, do not drop it at the end of the command body, or
/// the claim ends while the radio is still being written to.
pub fn claim(port: &str) -> Result<PortGuard, String> {
    let mut busy = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    if !busy.insert(port.to_string()) {
        return Err(format!(
            "A radio operation is already running on {port}. Wait for it to finish — \
             starting a second one now would talk over the first, and on a radio that \
             reboots when it commits, the port disappears mid-write. If you are sure \
             nothing is running, close and reopen the program dialog."
        ));
    }
    Ok(PortGuard {
        port: port.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_claim_on_the_same_port_is_refused_and_the_first_still_works() {
        let port = "/dev/cu.test-second-claim";
        let first = claim(port).expect("first claim");
        let err = claim(port).unwrap_err();
        assert!(err.contains(port), "{err}");
        assert!(err.contains("already running"), "{err}");
        drop(first);
        // Released — the recovery path after a failed write must not be locked
        // out by the write that failed.
        claim(port).expect("claimable again once the first is done");
    }

    #[test]
    fn a_different_port_is_not_blocked() {
        let a = claim("/dev/cu.test-radio-a").expect("a");
        let b = claim("/dev/cu.test-radio-b").expect("b");
        drop((a, b));
    }

    /// A panic inside an operation must free the port, or one crash wedges that
    /// radio for the life of the process.
    #[test]
    fn a_panicking_operation_releases_its_port() {
        let port = "/dev/cu.test-panic";
        let r = std::panic::catch_unwind(|| {
            let _guard = claim(port).expect("claim");
            panic!("the radio operation blew up");
        });
        assert!(r.is_err());
        claim(port).expect("the port is free again after the panic");
    }
}
