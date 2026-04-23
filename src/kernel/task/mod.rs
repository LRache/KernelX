pub mod def;
pub mod fdtable;
pub mod manager;
mod pcb;
pub mod pidfd;
mod tcb;

pub use manager::{create_initprocess, with_initpcb};
pub use pcb::*;
pub use tcb::*;

use manager::INIT_UTASK_TID;
