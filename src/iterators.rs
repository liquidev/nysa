//! Iterators over messages on the bus.

use std::marker::PhantomData;
use std::sync::{Arc, MutexGuard};

use crate::{Bus, DynMessage, Message, MessageStore};

/// An "ownership wrapper" over a bus's message store.
///
/// This is required such that the actual [`RetrieveAll`] can have an owned reference to the inner
/// data of the bus, and such, have a valid lifetime that can't outlive that data.
///
/// This type implements `IntoIterator`, so using it is as simple as
/// `for message in &`[`bus.retrieve_all`][Bus::retrieve_all]`::<T>()` (note the borrow).
pub struct RetrieveAllRef<'bus, T>
where
   T: 'static + Send,
{
   store: Arc<MessageStore>,
   _bus: PhantomData<&'bus Bus>,
   _data: PhantomData<T>,
}

impl<'bus, T> RetrieveAllRef<'bus, T>
where
   T: 'static + Send,
{
   pub(crate) fn new(store: Arc<MessageStore>) -> Self {
      Self {
         store,
         _bus: PhantomData,
         _data: PhantomData,
      }
   }
}

impl<'bus, 'r, T> IntoIterator for &'r RetrieveAllRef<'bus, T>
where
   T: 'static + Send,
{
   type Item = Message<'bus, T>;
   type IntoIter = RetrieveAll<'bus, 'r, T>;

   fn into_iter(self) -> Self::IntoIter {
      RetrieveAll {
         _rr: self,
         messages: self.store.messages.lock().unwrap(),
         index: 0,
      }
   }
}

/// An iterator over all messages of a given type on a bus.
pub struct RetrieveAll<'bus, 'r, T>
where
   T: 'static + Send,
{
   _rr: &'r RetrieveAllRef<'bus, T>,
   messages: MutexGuard<'r, Vec<Arc<DynMessage>>>,
   index: usize,
}

impl<'bus, T> Iterator for RetrieveAll<'bus, '_, T>
where
   T: 'static + Send,
{
   type Item = Message<'bus, T>;

   fn next(&mut self) -> Option<Self::Item> {
      loop {
         if self.index < self.messages.len() {
            let index = self.index;
            self.index += 1;
            if !self.messages[index].is_free() {
               let message = Message::new(Arc::clone(&self.messages[index]));
               return Some(message);
            }
         } else {
            return None;
         }
      }
   }
}
