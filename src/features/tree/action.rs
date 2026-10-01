//! Actions owned by the tree feature.
//!
//! The tree does not need actions of its own yet: the messages it reacts to
//! come from [`crate::app::action::Action`]. The module exists so that every
//! feature keeps the same four part shape.

/// Messages the tree feature sends to itself.
#[derive(Debug)]
pub enum TreeAction {
    /// The selection moved because the user asked for it.
    SelectionMoved { follow_broken: bool },
}
