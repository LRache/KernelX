pub mod context;
pub mod percpu;
pub mod switch;
pub mod traphandle;

pub use percpu::*;
pub use traphandle::*;

use crate::arch::riscv::csr::csrw_stvec;
use traphandle::kerneltrap_handler;

pub fn init() {
    csrw_stvec(kerneltrap_handler as usize);
}
