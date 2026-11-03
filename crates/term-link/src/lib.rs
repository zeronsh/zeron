//! Terminals on other devices, for any viewer — the desktop engine and the
//! phone run the same code:
//!
//! - [`link`]: the direct peer-to-peer link's framed protocol (both ends);
//! - [`remote`]: the hub carrying a terminal's I/O over the direct link when
//!   the devices can reach each other and the relay when they can't,
//!   switching mid-session with no gap, no repeat and no lost keystroke.

pub mod link;
pub mod remote;
