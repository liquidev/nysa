//! The implementation of the bus.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

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
   pub fn retrieve_all<'bus, T, I>(&'bus self, mut iter: I)
   where
      T: 'static + Send,
      I: FnMut(Message<'bus, T>),
   {
      let type_id = TypeId::of::<T>();
      let store = {
         let locked = self.inner.lock().unwrap();
         let mut borrowed = locked.borrow_mut();
         match borrowed.messages.get_mut(&type_id) {
            Some(store) => Arc::clone(store),
            _ => return,
         }
      };
      let messages = store.messages.lock().unwrap();
      for dyn_message in messages.iter() {
         let message = Message::new(Arc::clone(dyn_message));
         iter(message);
      }
   }

   /// Blocks execution until a message of the given type arrives on the bus.
   fn wait_for_impl<'bus, T>(&'bus self, timeout: Option<Duration>) -> Option<Message<'bus, T>>
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

/// A message on the bus.
///
/// Messages can be read, and then ignored, or consumed. A message handle must not outlive the bus
/// it was pushed to, hence the lifetime `'bus`.
pub struct Message<'bus, T>
where
   T: 'static + Send,
{
   message: Arc<DynMessage>,
   data: Option<Box<T>>,
   _phantom_data: PhantomData<&'bus BusInner>,
}

impl<'bus, T> Message<'bus, T>
where
   T: 'static + Send,
{
   /// Creates a new message with the given type, referring to the given message.
   ///
   /// This trusts that the message is of the given type.
   fn new(message: Arc<DynMessage>) -> Self {
      assert!(
         !message.is_borrowed.load(Ordering::SeqCst),
         "data race: cannot borrow a message twice"
      );
      message.is_borrowed.store(true, Ordering::SeqCst);
      let data = message.take().unwrap();
      Self {
         message,
         data: Some(data),
         _phantom_data: PhantomData,
      }
   }

   /// Consumes the message and returns its inner data.
   ///
   /// This removes the message from the bus, so subsequent calls to `receive_all` and `wait_for`
   /// will not yield this message.
   pub fn consume(mut self) -> T {
      *self.data.take().unwrap()
   }
}

impl<T> Deref for Message<'_, T>
where
   T: 'static + Send,
{
   type Target = T;

   fn deref(&self) -> &Self::Target {
      self.data.as_ref().unwrap()
   }
}

impl<T> Drop for Message<'_, T>
where
   T: 'static + Send,
{
   fn drop(&mut self) {
      if let Some(data) = self.data.take() {
         self.message.is_borrowed.store(false, Ordering::SeqCst);
         self.message.put(data);
      }
   }
}

/// A message on the bus, with its type erased.
struct DynMessage {
   // What a chain.
   data: Mutex<RefCell<Option<Box<dyn Any + Send>>>>,
   is_borrowed: AtomicBool,
   store: Weak<MessageStore>,
}

impl DynMessage {
   /// Boxes the provided message data into a message.
   fn new<T>(data: T, store: Weak<MessageStore>) -> DynMessage
   where
      T: 'static + Send,
   {
      Self {
         data: Mutex::new(RefCell::new(Some(Box::new(data)))),
         store,
         is_borrowed: AtomicBool::new(false),
      }
   }

   /// Returns whether the message has no data stored (has already been consumed).
   fn is_free(&self) -> bool {
      let locked = self.data.lock().unwrap();
      let borrowed = locked.borrow();
      !self.is_borrowed.load(Ordering::SeqCst) && borrowed.is_none()
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
         self.store.upgrade().unwrap().message_count.fetch_sub(1, Ordering::SeqCst);
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
      assert!(
         !self.is_borrowed.load(Ordering::SeqCst),
         "data race: attempt to put() a value in a borrowed message"
      );
      let locked = self.data.lock().unwrap();
      let mut borrowed = locked.borrow_mut();
      borrowed.replace(data);
   }
}

/// A store for messages of a single type.
struct MessageStore {
   messages: Mutex<Vec<Arc<DynMessage>>>,
   message_count: AtomicUsize,
   condvar: Condvar,
}

/// The bus's inner message store.
struct BusInner {
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

   /// Pushes a message onto the bus.
   fn push<T>(&mut self, message_data: T)
   where
      T: 'static + Send,
   {
      let type_id = TypeId::of::<T>();
      let store = self.get_or_create_message_store(type_id);
      let mut messages = store.messages.lock().unwrap();

      if let Some(message) = messages.iter_mut().find(|msg| msg.is_free()) {
         message.put(Box::new(message_data));
      } else {
         let message = Arc::new(DynMessage::new(message_data, Arc::downgrade(store)));
         messages.push(message);
      }
      store.message_count.fetch_add(1, Ordering::SeqCst);
      store.condvar.notify_one();
   }
}
