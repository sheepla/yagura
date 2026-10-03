//! The finder overlay: a query box, a list of files or of grep hits, and a
//! preview.

pub mod grep;
pub mod reducer;
pub mod state;
pub mod view;

pub use state::FinderState;
