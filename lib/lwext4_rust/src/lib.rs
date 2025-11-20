#![no_std]
#![feature(linkage)]
#![feature(c_variadic, c_size_t)]
#![feature(associated_type_defaults)]

extern crate alloc;

#[macro_use]
extern crate log;

// mod ulibc;

pub mod ffi {
    #![allow(non_upper_case_globals)]
    #![allow(non_camel_case_types)]
    #![allow(non_snake_case)]

    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

mod blockdev;
mod error;
mod fs;
mod inode;
mod util;

pub use blockdev::{BlockDevice, EXT4_DEV_BSIZE};
pub use error::{Ext4Error, Ext4Result};
pub use fs::*;
pub use inode::*;
