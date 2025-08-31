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
pub use initprocess::*;

pub fn init() {
    get_init_process();
}
