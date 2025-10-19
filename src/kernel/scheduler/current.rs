use alloc::sync::Arc;
use spin::Mutex;

use crate::kernel::ipc::SignalActionTable;
use crate::kernel::mm::AddrSpace;
use crate::kernel::task::tid::Tid;
use crate::kernel::task::{PCB, TCB};
use crate::kernel::task::fdtable::FDTable;
use crate::kernel::scheduler::Processor;
use crate::arch;
use crate::fs::Dentry;

// #[macro_export]
// macro_rules! copy_to_user {
//     ($uaddr:expr, $data:expr) => {
//         {
//             let data_ptr = &$data as *const _ as *const u8;
//             let data_size = core::mem::size_of_val(&$data);
//             let data_slice = unsafe { core::slice::from_raw_parts(data_ptr, data_size) };
//             $crate::kernel::scheduler::current::copy_to_user($uaddr, data_slice)
//         }
//     };
// }

// #[macro_export]
// macro_rules! copy_to_user_ref {
//     ($uaddr:expr, $data_ref:expr) => {
//         {
//             let data_ptr = $data_ref as *const _ as *const u8;
//             let data_size = core::mem::size_of_val($data_ref);
//             let data_slice = unsafe { core::slice::from_raw_parts(data_ptr, data_size) };
//             $crate::kernel::scheduler::current::copy_to_user($uaddr, data_slice)
//         }
//     };
// }

// #[macro_export]
// macro_rules! copy_slice_to_user {
//     ($uaddr:expr, $slice:expr) => {
//         {
//             let slice = $slice;
//             let byte_slice = unsafe {
//                 core::slice::from_raw_parts(
//                     slice.as_ptr() as *const u8,
//                     core::mem::size_of_val(slice),
//                 )
//             };
//             $crate::kernel::scheduler::current::copy_to_user($uaddr, byte_slice)
//         }
//     };
// }


// #[macro_export]
// macro_rules! copy_to_user_string {
//     ($uaddr:expr, $buf:expr, $size:expr) => {
//         (|| -> $crate::kernel::errno::SysResult<usize> {
//             let bytes = $buf.as_bytes();
//             let len = core::cmp::min(bytes.len(), $size - 1);
//             $crate::kernel::scheduler::current::copy_to_user($uaddr, bytes)?;
//             $crate::kernel::scheduler::current::copy_to_user($uaddr + len, &[0u8])?;
//             Ok(len)
//         })()
//     };
// }

// #[macro_export]
// macro_rules! copy_from_user {
//     ($uaddr:expr, $data:expr) => {
//         {
//             let data_ptr = &$data as *const _ as *mut u8;
//             let data_size = core::mem::size_of_val(&$data);
//             let data_slice = unsafe { core::slice::from_raw_parts_mut(data_ptr, data_size) };
//             $crate::kernel::scheduler::current::copy_from_user_buffer($uaddr, data_slice)
//         }
//     };
// }

pub fn processor() -> &'static mut Processor<'static> {
    let p = arch::get_percpu_data() as *mut Processor;
    
    debug_assert!(!p.is_null());
    
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

pub mod copy_to_user {
    use crate::kernel::errno::SysResult;
    use super::addrspace;

    pub fn buffer(uaddr: usize, buf: &[u8]) -> SysResult<()> {
        addrspace().copy_to_user_buffer(uaddr, buf)
    }

    pub fn object<T: Copy>(uaddr: usize, value: T) -> SysResult<()> {
        addrspace().copy_to_user_object(uaddr, value)
    }

    pub fn slice<T: Copy>(uaddr: usize, slice: &[T]) -> SysResult<()> {
        addrspace().copy_to_user_slice(uaddr, slice)
    }

    pub fn array<T: Copy, const N: usize>(uaddr: usize, arr: &[T; N]) -> SysResult<()> {
        addrspace().copy_to_user_array(uaddr, arr)
    }

    pub fn string(uaddr: usize, s: &str, max_size: usize) -> SysResult<usize> {
        let bytes = s.as_bytes();
        let len = core::cmp::min(bytes.len(), max_size - 1);
        addrspace().copy_to_user_buffer(uaddr, &bytes[..len])?;
        addrspace().copy_to_user_buffer(uaddr + len, &[0u8])?;
        Ok(len)
    }
}

pub mod copy_from_user {
    use alloc::string::String;
    use crate::kernel::errno::SysResult;
    use super::addrspace;

    pub fn buffer(uaddr: usize, buf: &mut [u8]) -> SysResult<()> {
        addrspace().copy_from_user_buffer(uaddr, buf)
    }

    pub fn object<T: Copy>(uaddr: usize) -> SysResult<T> {
        addrspace().copy_from_user::<T>(uaddr)
    }

    pub fn string(uaddr: usize) -> SysResult<String> {
        addrspace().get_user_string(uaddr)
    }

    pub fn slice<T: Copy>(uaddr: usize, slice: &mut [T]) -> SysResult<()> {
        addrspace().copy_from_user_slice(uaddr, slice)
    }
}

pub fn schedule() {
    processor().schedule();
}

pub fn block(reason: &'static str) {
    tcb().block(reason);
    schedule();
}
