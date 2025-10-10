#![allow(non_camel_case_types)]

mod types;

mod openflags;
mod dirent;

mod signum;
mod sigframe;
mod sigaction;

pub use openflags::*;
pub use dirent::*;

pub use signum::*;
pub use sigframe::*;
pub use sigaction::*;

pub type pid_t = i32;
pub type uid_t = u32;
pub type gid_t = u32;
pub type clock_t = i64;
