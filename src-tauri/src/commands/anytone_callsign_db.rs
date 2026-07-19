//! Thin re-export shim. The AnyTone AT-D890UV call-sign DB encoder moved into
//! the radio driver at `radios/anytone_atd890uv/callsign_db.rs` in Chunk 3.5;
//! this keeps the old `commands::anytone_callsign_db::*` path resolving for the
//! RE example binaries and `anytone_program.rs` until 3.6 rewires them.
pub use crate::radios::anytone_atd890uv::callsign_db::*;
