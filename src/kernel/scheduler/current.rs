use alloc::sync::Arc;
use alloc::string::String;
use spin::Mutex;

use crate::kernel::ipc::{PendingSignalQueue, SignalActionTable};
use crate::kernel::mm::AddrSpace;
use crate::kernel::task::tid::Tid;
use crate::kernel::task::{PCB, TCB};
use crate::kernel::task::fdtable::FDTable;
use crate::kernel::scheduler::Processor;
use crate::kernel::errno::Errno;
use crate::arch;
use crate::fs::Dentry;

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
macro_rules! copy_to_user_ref {
    ($uaddr:expr, $data_ref:expr) => {
        {
            let data_ptr = $data_ref as *const _ as *const u8;
            let data_size = core::mem::size_of_val($data_ref);
            let data_slice = unsafe { core::slice::from_raw_parts(data_ptr, data_size) };
            $crate::kernel::scheduler::current::copy_to_user($uaddr, data_slice)
        }
    };
}

#[macro_export]
macro_rules! copy_slice_to_user {
    ($uaddr:expr, $slice:expr) => {
        {
            let slice = $slice;
            let byte_slice = unsafe {
                core::slice::from_raw_parts(
                    slice.as_ptr() as *const u8,
                    core::mem::size_of_val(slice),
                )
            };
            $crate::kernel::scheduler::current::copy_to_user($uaddr, byte_slice)
        }
    };
}


#[macro_export]
macro_rules! copy_to_user_string {
    ($uaddr:expr, $buf:expr, $size:expr) => {
        (|| -> $crate::kernel::errno::SysResult<usize> {
            let bytes = $buf.as_bytes();
            let len = core::cmp::min(bytes.len(), $size - 1);
            $crate::kernel::scheduler::current::copy_to_user($uaddr, bytes)?;
            $crate::kernel::scheduler::current::copy_to_user($uaddr + len, &[0u8])?;
            Ok(len)
        })()
    };
}

#[macro_export]
macro_rules! copy_from_user {
    ($uaddr:expr, $data:expr) => {
        {
            let data_ptr = &$data as *const _ as *mut u8;
            let data_size = core::mem::size_of_val(&$data);
            let data_slice = unsafe { core::slice::from_raw_parts_mut(data_ptr, data_size) };
            $crate::kernel::scheduler::current::copy_from_user($uaddr, data_slice)
        }
    };
}

pub fn processor() -> &'static mut Processor<'static> {
    let p = arch::get_percpu_data() as *mut Processor;
    
    assert!(!p.is_null());
    
    unsafe { &mut *p }
}

pub fn set(p: *mut Processor) {
    arch::set_percpu_data(p as usize);
}

pub fn clear() {
    arch::set_percpu_data(0);
}

pub fn is_clear() -> bool {
    arch::get_percpu_data() == 0
}

pub fn tcb() -> &'static Arc<TCB> {
    let processor = processor();
    processor.tcb
}

pub fn tid() -> Tid {
    if is_clear() {
        -1
    } else {
        tcb().get_tid()
    }
}

pub fn pcb() -> &'static Arc<PCB> {
    let processor = processor();
    &processor.tcb.get_parent()
}

pub fn signal_actions() -> &'static Mutex<SignalActionTable> {
    let pcb = pcb();
    pcb.signal_actions()
}

pub fn addrspace() -> &'static Arc<AddrSpace> {
    let tcb = tcb();
    tcb.get_addrspace()
}

pub fn fdtable() -> &'static Mutex<FDTable> {
    let tcb = tcb();
    tcb.fdtable()
}

pub fn with_cwd<F, R>(f: F) -> R 
where F: FnOnce(&Arc<Dentry>) -> R {
    let pcb = pcb();
    pcb.with_cwd(f)
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
    processor().schedule();
}
