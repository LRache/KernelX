#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
#![feature(linked_list_cursors)]

extern crate alloc;

mod kernel;
mod klib;
mod fs;
mod driver;
mod arch;
// mod platform;
