//! The pure kernel of `prw`: no I/O, no async, no terminal, no clock (a time is always an
//! argument).
//!
//! Everything here is a function of its arguments, so it is tested with properties
//! (proptest) rather than examples. The `prw` binary does the I/O and calls in here for
//! every decision.
//!
//! - [`protocol`]: the shared files under `~/.cache/pr-watch/` (`results.json`, session
//!   files, the lease), parsed at the boundary: a torn file is an error or nothing, never a
//!   panic, and an odd field is absent.
//! - [`gh`]: a PR as `results.json` stores it (the mod's `GhPr`); [`time`]: its timestamps.
//! - [`layout`]: which arrangement of panes fits a terminal width.

pub mod gh;
pub mod layout;
mod lenient;
pub mod protocol;
pub mod time;

pub use layout::{Layout, layout};
pub use protocol::{
    Entry, Lease, ParseError, SessionFile, Shared, parse_lease, parse_session, parse_shared,
    urls_in,
};
