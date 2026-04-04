mod def;
mod event;
mod fs;
mod futex;
mod ipc;
mod misc;
mod mm;
mod task;
mod time;
mod uid;

mod num;
mod uptr;

pub use num::syscall;
pub use uptr::UserStruct;

use crate::kernel::errno::SysResult;

pub type Args = [usize; 7];
pub type SyscallRet = SysResult<usize>;
