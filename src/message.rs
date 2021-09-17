use std::any::Any;
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crate::{BusInner, MessageStore};

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
   pub(crate) fn new(message: Arc<DynMessage>) -> Self {
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
      let data = *self.data.take().unwrap();
      self.message.is_borrowed.store(false, Ordering::SeqCst);
      data
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
pub(crate) struct DynMessage {
   // What a chain.
   data: Mutex<Option<Box<dyn Any + Send>>>,
   is_borrowed: AtomicBool,
   store: Weak<MessageStore>,
}

impl DynMessage {
   /// Boxes the provided message data into a message.
   pub fn new<T>(data: T, store: Weak<MessageStore>) -> DynMessage
   where
      T: 'static + Send,
   {
      Self {
         data: Mutex::new(Some(Box::new(data))),
         store,
         is_borrowed: AtomicBool::new(false),
      }
   }

   /// Returns whether the message has no data stored (has already been consumed).
   pub fn is_free(&self) -> bool {
      let data = self.data.lock().unwrap();
      !self.is_borrowed.load(Ordering::SeqCst) && data.is_none()
   }

   /// Returns whether the message is of the given type. If the message data has already been
   /// [`consume`]d, returns `None`.
   pub fn is<T>(&self) -> bool
   where
      T: 'static + Send,
   {
      let data = self.data.lock().unwrap();
      match data.deref() {
         Some(x) => x.is::<T>(),
         None => false,
      }
   }

   /// If the stored data is of the provided type `T`, and hasn't been taken out yet, takes the data
   /// out of the message and returns `Some(data)`. Otherwise, returns `None`.
   pub fn take<T>(&self) -> Option<Box<T>>
   where
      T: 'static + Send,
   {
      if self.is::<T>() {
         let mut data = self.data.lock().unwrap();
         let boxed = data.take()?;
         self.store.upgrade().unwrap().message_count.fetch_sub(1, Ordering::SeqCst);
         Some(boxed.downcast::<T>().ok()?)
      } else {
         None
      }
   }

   /// Puts some data back into a message.
   pub fn put<T>(&self, data: Box<T>)
   where
      T: 'static + Send,
   {
      assert!(
         !self.is_borrowed.load(Ordering::SeqCst),
         "data race: attempt to put() a value in a borrowed message"
      );
      let mut locked = self.data.lock().unwrap();
      locked.replace(data);
   }
}
