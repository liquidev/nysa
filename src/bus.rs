//! The implementation of the bus.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::Duration;

const ATOMIC_ORDERING: Ordering = Ordering::SeqCst;

/// A bus for passing messages across threads.
///
/// Nysa buses are fully thread-safe, and thus, can be stored in `static` variables. The library
/// provides a "default" bus in the module [`crate::global`].
pub struct Bus {
   inner: Mutex<RefCell<BusInner>>,
}

impl Bus {
   /// Creates a new bus.
   pub fn new() -> Self {
      Self {
         inner: Mutex::new(RefCell::new(BusInner::new())),
      }
   }

   /// Pushes a message with the given data onto the bus.
   pub fn push<T>(&self, message_data: T)
   where
      T: 'static + Send,
   {
      let locked = self.inner.lock().unwrap();
      let mut borrowed = locked.borrow_mut();
      borrowed.push(message_data);
   }

   /// Retrieves all messages of the given type from the bus.
   ///
   /// Note that the bus for the given message type is locked for the entire duration of this loop.
   ///
   /// # See also
   /// - [`Bus::wait_for`]
   /// - [`Bus::wait_for_timeout`]
   pub fn retrieve_all<T>(&self, mut iter: impl FnMut(T))
   where
      T: 'static + Send,
   {
      // let type_id = TypeId::of::<T>();
      // let store_mutex = {
      //    let locked = self.inner.lock().unwrap();
      //    let mut borrowed = locked.borrow_mut();
      //    match borrowed.messages.get_mut(&type_id) {
      //       Some(store) => Arc::clone(store),
      //       _ => return,
      //    }
      // };
      // let mut store = store_mutex.lock().unwrap();
      // for message in store.messages.iter() {}
   }

   /// Blocks execution until a message of the given type arrives on the bus.
   fn wait_for_impl<T>(&self, timeout: Option<Duration>) -> Option<MessageToken<'_, T>>
   where
      T: 'static + Send,
   {
      let type_id = TypeId::of::<T>();

      let store = {
         let locked = self.inner.lock().unwrap();
         let mut borrowed = locked.borrow_mut();
         Arc::clone(borrowed.get_or_create_message_store(type_id))
      };
      let mut messages = store.messages.lock().unwrap();

      while messages.len() - store.freed_messages.lock().unwrap().len() == 0 {
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

      let token = MessageToken::new(message);
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
   pub fn wait_for<T>(&self) -> MessageToken<'_, T>
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
   pub fn wait_for_timeout<T>(&self, timeout: Duration) -> Option<MessageToken<'_, T>>
   where
      T: 'static + Send,
   {
      self.wait_for_impl(Some(timeout))
   }
}

/// A message token. This token is used to either consume, or ignore a message.
pub struct MessageToken<'m, T>
where
   T: 'static + Send,
{
   message: &'m Message,
   data: Option<Box<T>>,
}

impl<'m, T> MessageToken<'m, T>
where
   T: 'static + Send,
{
   /// Creates a new message token with the given type, referring to the given message.
   ///
   /// This trusts that the message is of the given type.
   fn new(message: &'m Message) -> Self {
      let data = message.take().unwrap();
      Self {
         message,
         data: Some(data),
      }
   }

   /// Consumes the message referred to by the token and returns its inner data.
   ///
   /// This removes the message from the bus, so subsequent calls to `receive_all` and `wait_for`
   /// will not yield this message.
   pub fn consume(mut self) -> T {
      *self.data.take().unwrap()
   }
}

impl<T> Deref for MessageToken<'_, T>
where
   T: 'static + Send,
{
   type Target = T;

   fn deref(&self) -> &Self::Target {
      self.data.as_ref().unwrap()
   }
}

impl<T> Drop for MessageToken<'_, T>
where
   T: 'static + Send,
{
   fn drop(&mut self) {
      let data = self.data.take().unwrap();
      self.message.put(data);
   }
}

/// A message on the bus.
struct Message {
   type_id: TypeId,
   // What a chain.
   data: Mutex<RefCell<Option<Box<dyn Any + Send>>>>,
   store: Weak<Messages>,
}

impl Message {
   /// Boxes the provided message data into a message.
   fn new<T>(data: T, store: Weak<Messages>) -> Message
   where
      T: 'static + Send,
   {
      Self {
         type_id: Any::type_id(&data),
         data: Mutex::new(RefCell::new(Some(Box::new(data)))),
         store,
      }
   }

   /// Returns whether the message is of the given type. If the message data has already been
   /// [`consume`]d, returns `None`.
   fn is<T>(&self) -> bool
   where
      T: 'static + Send,
   {
      let locked = self.data.lock().unwrap();
      let borrowed = locked.borrow();
      match borrowed.deref() {
         Some(x) => x.is::<T>(),
         None => false,
      }
   }

   /// If the stored data is of the provided type `T`, and hasn't been taken out yet, takes the data
   /// out of the message and returns `Some(data)`. Otherwise, returns `None`.
   fn take<T>(&self) -> Option<Box<T>>
   where
      T: 'static + Send,
   {
      if self.is::<T>() {
         let locked = self.data.lock().unwrap();
         let mut borrowed = locked.borrow_mut();
         let boxed = borrowed.take()?;
         // The Weak in the Message can't possibly outlive the Arc of the message store.
         let store_arc = self.store.upgrade().unwrap();
         Some(boxed.downcast::<T>().ok()?)
      } else {
         None
      }
   }

   /// Puts some data back into a message.
   fn put<T>(&self, data: Box<T>)
   where
      T: 'static + Send,
   {
      let locked = self.data.lock().unwrap();
      let mut borrowed = locked.borrow_mut();
      borrowed.replace(data);
   }
}

/// A store for messages of a single type.
struct Messages {
   messages: Mutex<Vec<Message>>,
   freed_messages: Mutex<Vec<usize>>,
   condvar: Condvar,
}

/// The bus's inner message store.
struct BusInner {
   messages: HashMap<TypeId, Arc<Messages>>,
}

impl BusInner {
   /// Creates a new inner bus.
   fn new() -> Self {
      Self {
         messages: HashMap::new(),
      }
   }

   /// Locks and returns a handle to the message store for the given type ID, creating a new message
   /// store if it doesn't already exist.
   fn get_or_create_message_store(&mut self, type_id: TypeId) -> &Arc<Messages> {
      self.messages.entry(type_id).or_insert_with(|| {
         Arc::new(Messages {
            messages: Mutex::new(Vec::new()),
            freed_messages: Mutex::new(Vec::new()),
            condvar: Condvar::new(),
         })
      })
   }

   /// Locks and returns a handle to a message store for the given type ID. Panics if the message
   /// store for the type ID does not exist.
   fn get_message_store(&mut self, type_id: TypeId) -> &Arc<Messages> {
      self.messages.get(&type_id).unwrap()
   }

   /// Pushes a message onto the bus.
   fn push<T>(&mut self, message_data: T)
   where
      T: 'static + Send,
   {
      let type_id = TypeId::of::<T>();
      let store = self.get_or_create_message_store(type_id);
      let mut messages = store.messages.lock().unwrap();
      messages.push(Message::new(message_data, Arc::downgrade(store)));
      store.condvar.notify_one();
   }
}
