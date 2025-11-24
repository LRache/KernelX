mod task;
mod mm;
mod fs;
mod misc;
mod time;
mod event;
mod ipc;
mod uid;
mod futex;
mod def;

mod num;
mod uptr;

pub use num::syscall;
pub use uptr::UserStruct;

use crate::kernel::errno::SysResult;

pub type Args = [usize; 7];
pub type SyscallRet = SysResult<usize>;
