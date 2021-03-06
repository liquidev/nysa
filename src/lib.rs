//! A bus for passing messages around between independent subsystems of an application.

mod bus;
mod message;

#[cfg(feature = "global")]
pub mod global;
pub mod iterators;

pub use bus::*;
pub use message::*;
