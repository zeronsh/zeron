//! Block-level markdown over pulldown-cmark, shared by every frontend.
//!
//! - [`parser`] builds a [`BlockTree`] of top-level blocks with source ranges;
//!   [`IncrementalParser`] reparses only from the last stable top-level block
//!   boundary, so a streamed delta costs O(delta + last block).
//! - [`mend`] auto-closes hanging inline markers in the streaming *display*
//!   parse only; the canonical tree settles honestly on completion.

pub mod mend;
pub mod parser;

pub use parser::{Block, BlockTree, IncrementalParser, InlineRun, InlineStyle, TopBlock, parse_full};
