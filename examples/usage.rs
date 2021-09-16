// The following examples show how to use nysa buses for simple task orchestration between threads.

use std::{sync::Arc, time::Duration};

use nysa::Bus;

// First, let's define some types.

// Add will be our request to add two numbers, or end the adder thread.
enum Add {
   Two(i32, i32),
   Quit,
}

// AdditionResults will be pushed from the adder thread to the bus, as a result of addition.
struct AdditionResult(i32);

// This example demonstrates how to use an explicitly-created bus.
fn local_bus() {
   // The bus needs to be behind an `Arc`, such that we can share its instance between threads.
   // Access to buses is inherently gated behind a mutex, so all we need to do is share the data.
   let bus = Arc::new(Bus::new());

   let adder = {
      // Now we can spawn the adder thread. We need to clone the Arc such that we can send it into
      // the thread, while still preserving our original instance.
      let bus = Arc::clone(&bus);
      std::thread::spawn(move || loop {
         // The thread will wait indefinitely until it receives an Add message. It's good practice
         // to specify which message you want to receive explicitly, as type inference can hide
         // that information away.
         match bus.wait_for::<Add>().consume() {
            Add::Two(a, b) => bus.push(AdditionResult(a + b)),
            Add::Quit => break,
         }
      })
   };

   // Now that the adder thread is spinned up and ready to go, we'll send it some requests to
   // add numbers. The requests will be delayed by a second, to demonstrate that the thread will
   // indeed block until new messages arrive.
   bus.push(Add::Two(1, 2));
   std::thread::sleep(Duration::from_secs(1));
   bus.push(Add::Two(4, 5));
   std::thread::sleep(Duration::from_secs(1));
   // After all these requests, we'll send a final shutdown request, and wait until the thread
   // finishes execution.
   bus.push(Add::Quit);
   adder.join().unwrap();

   // Now we can take a look at our results and print them all out. Note that while `retrieve_all`
   // is performing its job of draining the message queue, the bus for the given type of messages
   // is locked and new messages will not be pushed until the loop finishes.
   bus.retrieve_all::<AdditionResult, _>(|message| {
      let AdditionResult(x) = message.consume();
      println!("{}", x);
   });
}

// Now, let's go over the same example, but using a global bus.
fn global_bus() {
   use nysa::global as bus;

   let adder = std::thread::spawn(move || loop {
      // We use `nysa::global::wait_for` to retrieve messages from the global bus.
      match bus::wait_for::<Add>().consume() {
         Add::Two(a, b) => bus::push(AdditionResult(a + b)),
         Add::Quit => break,
      }
   });

   // We use `nysa::global::push` to push messages to the global bus.
   bus::push(Add::Two(1, 2));
   std::thread::sleep(Duration::from_secs(1));
   bus::push(Add::Two(4, 5));
   std::thread::sleep(Duration::from_secs(1));
   bus::push(Add::Quit);
   adder.join().unwrap();

   // We use `nysa::global::retrieve_all` to retrieve all messages of a given type from the bus.
   bus::retrieve_all::<AdditionResult, _>(|message| {
      let AdditionResult(x) = message.consume();
      println!("{}", x);
   });
}

fn main() {
   local_bus();
   global_bus();
}
