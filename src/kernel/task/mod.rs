mod tcb;
mod pcb;
pub mod manager;
pub mod fdtable;
pub mod tid;
pub mod kernelstack;
pub mod def;

pub use tcb::*;
pub use pcb::*;
pub use tid::{Pid, Tid};
pub use kernelstack::*;
pub use manager::get_initprocess;

pub fn init() {
    manager::create_initprocess();
}
