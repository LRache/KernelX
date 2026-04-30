pub mod def;
pub mod fdtable;
pub mod manager;
mod pcb;
pub mod pidfd;
mod tcb;
mod uts;

pub use manager::{create_initprocess, with_initpcb};
pub use pcb::*;
pub use tcb::*;
pub use uts::{UTS_NAME_MAX, UtsNamespace};

use manager::INIT_UTASK_TID;
