//! A bus for passing messages around between independent subsystems of an application.

mod bus;
#[cfg(feature = "global")]
pub mod global;

pub use bus::*;
