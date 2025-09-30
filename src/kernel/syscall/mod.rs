mod task;
mod mm;
mod fs;
mod signal;
mod misc;
mod num;
mod time;
mod event;
mod ipc;
mod uid;
mod def;

pub use num::syscall;

use crate::kernel::errno::SysResult;

pub type Args = [usize; 7];
pub type SyscallRet = SysResult<usize>;
