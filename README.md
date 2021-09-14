# Nysa

A bus for passing messages around between independent subsystems of an application.

```rs
use std::time::Duration;

use nysa::global as bus;

enum Add {
   Two(i32, i32),
   Quit,
}

struct AdditionResult(i32);

fn main() {
   let adder = std::thread::spawn(move || loop {
      match bus::wait_for::<Add>() {
         Add::Two(a, b) => bus::push(AdditionResult(a + b)),
         Add::Quit => break,
      }
   });

   bus::push(Add::Two(1, 2));
   std::thread::sleep(Duration::from_secs(1));
   bus::push(Add::Two(4, 5));
   std::thread::sleep(Duration::from_secs(1));
   bus::push(Add::Quit);
   adder.join().unwrap();

   bus::retrieve_all(|AdditionResult(x)| {
      println!("{}", x);
   });
}
```

## What is nysa?

Nysa is a thread-safe message bus for applications.

It exposes a safe and simple to use API while abstracting away all the dirty details of mutexes and
condvars.

The main usage of nysa is desktop applications which rely on a lot of subsystems communicating with
each other to download, process, convert, compute data. Each subsystem can wait for messages to
arrive on the bus, and then push more messages onto the bus.

The core idea of nysa is to _keep it simple, stupid_. For nysa, code readability is more important
than blazing fast performance. Making an application use multiple threads should not be a very
difficult task.

This is also why nysa exposes a "default", static bus, available in the `nysa::global` module.
Using this bus alone should be enough for most applications, but if performance ever becomes a
problem, it's possible to create multiple smaller buses for orchestrating sub-subsystems in
subsystems.

## What nysa is not

- Nysa is not suited very well for I/O-intensive tasks. For that, you should use an async dispatcher
  such as [tokio](https://github.com/tokio-rs/tokio).
- Nysa is not production-grade software. It's a toy project built as a component of
  my collaborative painting app, [NetCanv](https://github.com/liquidev/netcanv). Maybe someday it'll
  come to a point where I'll be confident to release version 1.0, but I doubt it ever will.

## What's in the name?

The name is a throwback to an old Polish van, [ZSD Nysa](https://en.wikipedia.org/wiki/ZSD_Nysa),
manufactured back in the times of the Polish People's Republic. One of its variants was a minibus,
and that's what this library's name refers to.
