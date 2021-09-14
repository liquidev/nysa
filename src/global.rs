//! A shared bus for applications that don't need more than one. Comes with some utility methods
//! for simplified usage.

use std::time::Duration;

use lazy_static::lazy_static;

use crate::Bus;

lazy_static! {
   static ref BUS: Bus = Bus::new();
}

/// Pushes a message onto the global bus.
pub fn push<T>(message_data: T)
where
   T: 'static + Send,
{
   BUS.push(message_data);
}

/// Retrieves all messages of the given type from the global bus.
pub fn retrieve_all<T>(iter: impl FnMut(T))
where
   T: 'static + Send,
{
   BUS.retrieve_all(iter);
}

/// Blocks execution in the current thread until a message of the provided type arrives on the
/// bus, or the given timeout is reached.
pub fn wait_for_timeout<T>(timeout: Duration) -> Option<T>
where
   T: 'static + Send,
{
   BUS.wait_for_timeout(timeout)
}

/// Blocks execution in the current thread indefinitely until a message of the given type is
/// available on the global bus.
pub fn wait_for<T>() -> T
where
   T: 'static + Send,
{
   BUS.wait_for()
}
