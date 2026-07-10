#![allow(non_camel_case_types)]

mod dirent;
mod fallocate;
mod filestat;
mod memfd;
mod openflags;
mod sigaction;
mod statfs;

pub use dirent::*;
pub use fallocate::*;
pub use filestat::*;
pub use memfd::*;
pub use openflags::*;
pub use sigaction::*;
pub use statfs::*;

pub type uid_t = u32;
pub type Uid = u32;
