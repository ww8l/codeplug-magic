//! Live-USB radio drivers, organized as one folder per radio behind a shared
//! trait. See `codeplug-magic-launch-plan.md` Chunk 3.
//!
//! - `driver`   — the `RadioDriver` trait + capability sub-traits (3.1).
//! - `registry` — static driver list + model→driver lookup (3.2).
//! - `<key>/`   — one concrete driver per radio (3.3 UV-5R done; 3.4 TD-H3 and
//!   3.5 AnyTone D890UV still live under `commands/`).

pub(crate) mod baofeng_uv5r;
pub(crate) mod driver;
pub(crate) mod registry;
