#![no_std]
#![no_main]
#![feature(linked_list_cursors)]
#![feature(linked_list_retain)]

extern crate alloc;

mod arch;
mod driver;
mod fs;
mod kernel;
mod klib;
mod kmodule;
#[cfg(feature = "kvm")]
mod kvm;
mod net;
// mod platform;
