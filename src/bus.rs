//! The implementation of the bus.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::iterators::RetrieveAllRef;
use crate::{DynMessage, Message};

/// A bus for passing messages across threads.
///
/// Nysa buses are fully thread-safe, and thus, can be stored in `static` variables. The library
/// provides a "default" bus in the module [`crate::global`].
pub struct Bus {
   inner: Mutex<BusInner>,
}

impl Bus {
   /// Creates a new bus.
   pub fn new() -> Self {
      Self {
         inner: Mutex::new(BusInner::new()),
      }
   }

   /// Pushes a message with the given data onto the bus.
   pub fn push<T>(&self, message_data: T)
   where
      T: 'static + Send,
   {
      let type_id = TypeId::of::<T>();
      let mut inner = self.inner.lock().unwrap();
      let store = Arc::clone(inner.get_or_create_message_store(type_id));
      drop(inner);
      store.push(message_data);
   }

   /// Retrieves all messages of the given type from the bus.
   ///
   /// Note that the bus for the given message type is locked for the entire duration of this loop.
   ///
   /// # See also
   /// - [`Bus::wait_for`]
   /// - [`Bus::wait_for_timeout`]
   pub fn retrieve_all<'bus, T>(&'bus self) -> RetrieveAllRef<'bus, T>
   where
      T: 'static + Send,
   {
      let type_id = TypeId::of::<T>();
      let mut inner = self.inner.lock().unwrap();
      let store = inner.get_or_create_message_store(type_id);
      RetrieveAllRef::new(Arc::clone(store))
   }

   /// Blocks execution until a message of the given type arrives on the bus.
   fn wait_for_impl<'bus, T>(&'bus self, timeout: Option<Duration>) -> Option<Message<'bus, T>>
   where
      T: 'static + Send,
   {
      let type_id = TypeId::of::<T>();

      let store = {
         let mut inner = self.inner.lock().unwrap();
         let arc = Arc::clone(inner.get_or_create_message_store(type_id));
         drop(inner);
         arc
      };
      let mut messages = store.messages.lock().unwrap();

      while store.message_count.load(Ordering::SeqCst) == 0 {
         match timeout {
            Some(duration) => {
               let (mguard, timeout) = store.condvar.wait_timeout(messages, duration).unwrap();
               messages = mguard;
               if timeout.timed_out() {
                  return None;
               }
            }
            None => {
               messages = store.condvar.wait(messages).unwrap();
            }
         }
      }
      let mut dyn_message = None;
      for msg in messages.iter() {
         if msg.is::<T>() {
            dyn_message = Some(msg);
            break;
         }
      }
      let dyn_message = dyn_message.unwrap();

      let token = Message::new(Arc::clone(dyn_message));
      Some(token)
   }

   /// Blocks execution in the current thread indefinitely until a message of the provided type
   /// arrives on the bus.
   ///
   /// Although the Rust style guidelines say otherwise, to prevent bugs and aid readability it's
   /// best to always use the turbofish syntax over implicit type inference with this function, such
   /// that it's immediately visible what type of message is being waited for.
   ///
   /// # See also
   /// - [`Bus::retrieve_all`]
   /// - [`Bus::wait_for_timeout`]
   pub fn wait_for<'bus, T>(&'bus self) -> Message<'bus, T>
   where
      T: 'static + Send,
   {
      self.wait_for_impl(None).unwrap()
   }

   /// Blocks execution in the current thread until a message of the provided type arrives on the
   /// bus, or the given timeout is reached.
   ///
   /// This function will block for the specified amount of time at most and resume execution
   /// normally. Otherwise it will block indefinitely.
   ///
   /// Returns `Some(data)` if data was successfully fetched, or `None` if the timeout was reached.
   ///
   /// # See also
   /// - [`Bus::retrieve_all`]
   /// - [`Bus::wait_for`]
   pub fn wait_for_timeout<'bus, T>(&'bus self, timeout: Duration) -> Option<Message<'bus, T>>
   where
      T: 'static + Send,
   {
      self.wait_for_impl(Some(timeout))
   }
}

/// A store for messages of a single type.
pub(crate) struct MessageStore {
   pub messages: Mutex<Vec<Arc<DynMessage>>>,
   pub message_count: AtomicUsize,
   pub condvar: Condvar,
}

impl MessageStore {
   /// Pushes a message into the store.
   fn push<T>(self: Arc<Self>, message_data: T)
   where
      T: 'static + Send,
   {
      let mut messages = self.messages.lock().unwrap();

      if let Some(message) = messages.iter().find(|msg| msg.is_free()) {
         message.put(Box::new(message_data));
      } else {
         let message = Arc::new(DynMessage::new(message_data, Arc::downgrade(&self)));
         messages.push(message);
      }
      self.message_count.fetch_add(1, Ordering::SeqCst);
      self.condvar.notify_one();
   }
}

/// The bus's inner message store.
pub(crate) struct BusInner {
   messages: HashMap<TypeId, Arc<MessageStore>>,
}

impl BusInner {
   /// Creates a new inner bus.
   fn new() -> Self {
      Self {
         messages: HashMap::new(),
      }
   }

   /// Returns a handle to the message store for the given type ID, creating a new message
   /// store if it doesn't already exist.
   fn get_or_create_message_store(&mut self, type_id: TypeId) -> &Arc<MessageStore> {
      self.messages.entry(type_id).or_insert_with(|| {
         Arc::new(MessageStore {
            messages: Mutex::new(Vec::new()),
            message_count: AtomicUsize::new(0),
            condvar: Condvar::new(),
         })
      })
   }
}
