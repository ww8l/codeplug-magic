//! Live-USB radio drivers, organized as one folder per radio behind a shared
//! trait. See `codeplug-magic-launch-plan.md` Chunk 3.
//!
//! - `driver`   — the `RadioDriver` trait + capability sub-traits (3.1).
//! - `registry` — static driver list + model→driver lookup (3.2, TODO).
//! - `<key>/`   — one concrete driver per radio (3.3–3.5, TODO).

pub(crate) mod driver;
pub(crate) mod registry;
