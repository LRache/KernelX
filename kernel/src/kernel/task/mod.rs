mod tcb;
mod pcb;
mod initprocess;
pub mod fdtable;
pub mod tid;
pub mod kernelstack;
pub mod def;

pub use tcb::*;
pub use pcb::*;
pub use kernelstack::*;
pub use initprocess::get_initprocess;

pub fn init() {
    initprocess::create_initprocess();
}
