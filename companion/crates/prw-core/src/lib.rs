//! The pure kernel of `prw`: no I/O, no async, no terminal.
//!
//! Everything here is a function of its arguments, so it is tested with
//! properties (proptest) rather than examples. The `prw` binary does the I/O
//! and calls in here for every decision.

pub mod layout;

pub use layout::{Layout, layout};
