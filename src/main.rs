#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
#![feature(linked_list_cursors)]

extern crate alloc;

mod arch;
mod driver;
mod fs;
mod kernel;
mod klib;
// mod platform;
