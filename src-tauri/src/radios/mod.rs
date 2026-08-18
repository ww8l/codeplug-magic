//! Live-USB radio drivers, organized as one folder per radio behind a shared
//! trait. See `codeplug-magic-launch-plan.md` Chunk 3.
//!
//! - `driver`   — the `RadioDriver` trait + capability sub-traits (3.1).
//! - `registry` — static driver list + model→driver lookup (3.2).
//! - `<key>/`   — one concrete driver per radio (3.3 UV-5R + 3.4 TD-H3 done;
//!   3.5 AnyTone D890UV still lives under `commands/`).
//!
//! `yaesu_ft5d` is the first driver added after Chunk 3 and is scaffolding
//! only — registered and identifiable, but advertising no capabilities until a
//! programming modality is proven against the radio (issue #32).
//!
//! `icom_id52` is the second (issue #38), and the second radio programmed from
//! a memory card rather than a cable. Its `.icf` container is finished; its
//! memory map is waiting on dumps from the radio.
//!
//! `kenwood_thd75` is the third card radio (issue #40) and the first Kenwood.
//! Its `.d75` layout came from CHIRP's `thd74.py`, verified against a save off
//! the radio before any of it was written.

pub(crate) mod anytone_atd890uv;
pub(crate) mod baofeng_uv5r;
pub(crate) mod driver;
pub(crate) mod icom_id52;
pub(crate) mod kenwood_thd75;
pub(crate) mod registry;
pub(crate) mod tidradio_tdh3;
pub(crate) mod yaesu_ft5d;
