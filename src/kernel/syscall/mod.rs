mod common;
mod def;
mod event;
mod fs;
mod futex;
mod ipc;
mod misc;
mod mm;
mod socket;
mod task;
mod time;
mod uid;
mod utils;

mod num;
mod uptr;

pub use num::{should_restart_on_eintr, syscall};
pub(crate) use uid::{Capability, capable};
pub use uptr::UserStruct;

use crate::kernel::errno::SysResult;

pub type Args = [usize; 7];
pub type SyscallRet = SysResult<usize>;
