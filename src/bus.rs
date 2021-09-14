//! The implementation of the bus.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ops::{ControlFlow, Deref};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

const ATOMIC_ORDERING: Ordering = Ordering::Relaxed;

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
   pub fn retrieve_all<T>(&self, mut iter: impl FnMut(T) -> ControlFlow<()>)
   where
      T: 'static + Send,
   {
      let type_id = TypeId::of::<T>();
      let store_mutex = {
         let locked = self.inner.lock().unwrap();
         let mut borrowed = locked.borrow_mut();
         match borrowed.messages.get_mut(&type_id) {
            Some(store) => Arc::clone(store),
            _ => return,
         }
      };
      let mut store = store_mutex.lock().unwrap();
      for message in store.messages.drain(..) {
         if let Some(data) = message.consume() {
            match iter(data) {
               ControlFlow::Continue(()) => (),
               ControlFlow::Break(()) => break,
            }
         }
      }
   }

   /// Blocks execution until a message of the given type arrives on the bus.
   fn wait_for_impl<T>(&self, timeout: Option<Duration>) -> Option<T>
   where
      T: 'static + Send,
   {
      let type_id = TypeId::of::<T>();

      let mut locked = self.inner.lock().unwrap();
      let wakeup = {
         let mut borrowed = locked.borrow_mut();
         let store = borrowed.get_or_create_message_store(type_id);
         Arc::clone(&store.wakeup)
      };

      while wakeup.count.load(ATOMIC_ORDERING) == 0 {
         match timeout {
            Some(duration) => {
               let (mguard, timeout) = wakeup.condvar.wait_timeout(locked, duration).unwrap();
               locked = mguard;
               if timeout.timed_out() {
                  return None;
               }
            }
            None => {
               locked = wakeup.condvar.wait(locked).unwrap();
            }
         }
      }
      let mut borrowed = locked.borrow_mut();
      let mut store = borrowed.get_message_store(type_id);
      let message = store.messages.pop_front().unwrap();

      Some(message.consume::<T>().unwrap())
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
   pub fn wait_for<T>(&self) -> T
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
   pub fn wait_for_timeout<T>(&self, timeout: Duration) -> Option<T>
   where
      T: 'static + Send,
   {
      self.wait_for_impl(Some(timeout))
   }
}

/// A message on the bus.
#[derive(Debug)]
struct Message {
   type_id: TypeId,
   // What a chain.
   data: Mutex<RefCell<Option<Box<dyn Any + Send>>>>,
   wakeup: Arc<Wakeup>,
}

impl Message {
   /// Boxes the provided message data into a message.
   fn new<T>(data: T, wakeup: Arc<Wakeup>) -> Message
   where
      T: 'static + Send,
   {
      Self {
         type_id: Any::type_id(&data),
         data: Mutex::new(RefCell::new(Some(Box::new(data)))),
         wakeup,
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

   /// If the stored data is of the provided type `T`, and hasn't been consumed yet, takes the data
   /// out of the message and returns `Some(data)`. Otherwise, returns `None`.
   fn consume<T>(&self) -> Option<T>
   where
      T: 'static + Send,
   {
      if self.is::<T>() {
         let locked = self.data.lock().unwrap();
         let mut borrowed = locked.borrow_mut();
         let boxed = borrowed.take()?;
         self.wakeup.count.fetch_sub(1, ATOMIC_ORDERING);
         Some(*boxed.downcast::<T>().ok()?)
      } else {
         None
      }
   }
}

/// Wakeup information for a given message type.
#[derive(Debug)]
struct Wakeup {
   condvar: Condvar,
   count: AtomicUsize,
}

/// A store for messages of a single type.
struct Messages {
   messages: VecDeque<Message>,
   wakeup: Arc<Wakeup>,
}

/// The bus's inner message store.
struct BusInner {
   messages: HashMap<TypeId, Arc<Mutex<Messages>>>,
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
   fn get_or_create_message_store(&mut self, type_id: TypeId) -> MutexGuard<'_, Messages> {
      self
         .messages
         .entry(type_id)
         .or_insert_with(|| {
            Arc::new(Mutex::new(Messages {
               messages: VecDeque::new(),
               wakeup: Arc::new(Wakeup {
                  condvar: Condvar::new(),
                  count: AtomicUsize::new(0),
               }),
            }))
         })
         .lock()
         .unwrap()
   }

   /// Locks and returns a handle to a message store for the given type ID. Panics if the message
   /// store for the type ID does not exist.
   fn get_message_store(&mut self, type_id: TypeId) -> MutexGuard<'_, Messages> {
      self.messages.get_mut(&type_id).unwrap().lock().unwrap()
   }

   /// Pushes a message onto the bus.
   fn push<T>(&mut self, message_data: T)
   where
      T: 'static + Send,
   {
      let type_id = TypeId::of::<T>();
      let mut store = self.get_or_create_message_store(type_id);
      let wakeup = Arc::clone(&store.wakeup);
      store.messages.push_back(Message::new(message_data, wakeup));
      store.wakeup.count.fetch_add(1, ATOMIC_ORDERING);
      store.wakeup.condvar.notify_one();
   }
}
