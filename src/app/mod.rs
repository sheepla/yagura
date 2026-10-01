//! The Flux loop: actions, state, a pure reducer and effects.
//!
//! [`store::Store`] is the only place that owns the state. It applies actions
//! through [`reduce::reduce`], hands the resulting effects to the
//! [`runner::EffectRunner`] and draws the frames.

pub mod action;
pub mod command;
pub mod effect;
pub mod reduce;
pub mod runner;
pub mod state;
pub mod store;
pub mod view;

pub use action::Action;
pub use command::Command;
pub use effect::Effect;
pub use state::AppState;
