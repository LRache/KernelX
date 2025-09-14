use alloc::sync::Arc;
use alloc::string::String;

use crate::arch;
use crate::fs::Dentry;
use crate::kernel::mm::AddrSpace;
use crate::kernel::task::tid::Tid;
use crate::kernel::task::{PCB, TCB};
use crate::kernel::task::fdtable::FDTable;
use crate::kernel::errno::Errno;

#[macro_export]
macro_rules! copy_to_user {
    ($uaddr:expr, $data:expr) => {
        {
            let data_ptr = &$data as *const _ as *const u8;
            let data_size = core::mem::size_of_val(&$data);
            let data_slice = unsafe { core::slice::from_raw_parts(data_ptr, data_size) };
            $crate::kernel::scheduler::current::copy_to_user($uaddr, data_slice)
        }
    };
}

#[macro_export]
macro_rules! copy_from_user {
    ($uaddr:expr, $data:expr) => {
        {
            let data_ptr = $data as *mut _ as *mut u8;
            let data_size = core::mem::size_of_val($data);
            let data_slice = unsafe { core::slice::from_raw_parts_mut(data_ptr, data_size) };
            $crate::kernel::scheduler::current::copy_from_user($uaddr, data_slice)
        }
    };
}

pub fn tcb() -> &'static Arc<TCB> {
    let processor = arch::current_processor();
    processor.tcb
}

pub fn tid() -> Tid {
    tcb().get_tid()
}

pub fn pcb() -> &'static Arc<PCB> {
    let processor = arch::current_processor();
    &processor.tcb.get_parent()
}

pub fn addrspace() -> &'static Arc<AddrSpace> {
    let tcb = tcb();
    tcb.get_addrspace()
}

pub fn fdtable() -> &'static FDTable {
    let tcb = tcb();
    tcb.get_fd_table()
}

pub fn with_pwd<F, R>(f: F) -> R 
where F: FnOnce(&Arc<Dentry>) -> R {
    let pcb = pcb();
    pcb.with_pwd(f)
}

pub fn copy_to_user(uaddr: usize, buf: &[u8]) -> Result<(), Errno> {
    addrspace().copy_to_user(uaddr, buf)
}

pub fn copy_from_user(uaddr: usize, buf: &mut [u8]) -> Result<(), Errno> {
    addrspace().copy_from_user(uaddr, buf)
}

pub fn get_user_string(uaddr: usize) -> Result<String, Errno> {
    addrspace().get_user_string(uaddr)
}

pub fn schedule() {
    arch::current_processor().schedule();
}
